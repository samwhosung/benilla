//! Loader tests.

mod loader_tests {
    use crate::framexml;
    use crate::loader::*;
    use crate::order::ZTarget;
    use crate::script::{QuadContent, UiScript};

    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    fn parse(text: &str) -> framexml::ParsedDocument {
        framexml::parse(text).expect("valid FrameXML")
    }

    /// `CMinimap::LoadXML 0x4ee2b0`: `minimapArrowModel` goes to engine children 1-8 (`0x4ee170`),
    /// `minimapPlayerModel` to child 9 alone (`[Minimap+0x338]`, `0x4ee260`); without the
    /// attributes the defaults are `0x84c768` and `0x8453c0`. `Minimap.xml:97` sets both.
    #[test]
    fn the_minimap_model_attributes_name_the_nine_engine_children() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Minimap name="Minimap"
                         minimapArrowModel="Interface\Minimap\Rotating-MinimapArrow.mdl"
                         minimapPlayerModel="Interface\Minimap\MinimapArrow.mdl">
                    <Size><AbsDimension x="140" y="140"/></Size>
                    <Anchors><Anchor point="TOPRIGHT"/></Anchors>
                    <Frames>
                        <Frame name="MiniMapPingHost"/>
                    </Frames>
                </Minimap>
                <Minimap name="BareMinimap">
                    <Size><AbsDimension x="140" y="140"/></Size>
                    <Anchors><Anchor point="TOPLEFT"/></Anchors>
                </Minimap>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        for i in 1..=8 {
            assert_eq!(
                s.eval::<String>(&format!(
                    "return ({{Minimap:GetChildren()}})[{i}]:GetModel()"
                ))
                .unwrap(),
                "Interface\\Minimap\\Rotating-MinimapArrow.mdl",
                "engine child {i} takes minimapArrowModel"
            );
        }
        assert_eq!(
            s.eval::<String>("return ({Minimap:GetChildren()})[9]:GetModel()")
                .unwrap(),
            "Interface\\Minimap\\MinimapArrow.mdl",
            "child 9 — [Minimap+0x338] — takes minimapPlayerModel alone"
        );

        // The `<Frames>` child comes after the nine: the factory runs before the descent
        // (`0x6ee408` before `0x6ee4ea`) and both append at the tail.
        assert_eq!(
            s.eval::<String>("return ({Minimap:GetChildren()})[10]:GetName()")
                .unwrap(),
            "MiniMapPingHost"
        );

        assert_eq!(
            s.eval::<String>("return ({BareMinimap:GetChildren()})[1]:GetModel()")
                .unwrap(),
            crate::widget::MINIMAP_DEFAULT_ARROW_MODEL
        );
        assert_eq!(
            s.eval::<String>("return ({BareMinimap:GetChildren()})[9]:GetModel()")
                .unwrap(),
            crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL
        );
    }

    /// The stock FrameXML attaches with `parent=`; a `relativeTo`-less anchor measures from it.
    #[test]
    fn a_top_level_parent_attribute_attaches_and_anchors() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="Host">
                    <Size><AbsDimension x="200" y="100"/></Size>
                    <Anchors>
                        <Anchor point="TOPLEFT" relativePoint="TOPLEFT">
                            <Offset><AbsDimension x="100" y="-50"/></Offset>
                        </Anchor>
                    </Anchors>
                </Frame>
                <Frame name="Attached" parent="Host">
                    <Size><AbsDimension x="20" y="20"/></Size>
                    <Anchors>
                        <Anchor point="TOPLEFT">
                            <Offset><AbsDimension x="5" y="-5"/></Offset>
                        </Anchor>
                    </Anchors>
                </Frame>
                <Frame name="Orphan" parent="NoSuchFrame">
                    <Size><AbsDimension x="10" y="10"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s.resolve();

        assert_eq!(
            s.eval::<String>("return Attached:GetParent():GetName()")
                .unwrap(),
            "Host",
            "the attribute supplied the parent"
        );
        // Host's TOPLEFT is screen (100, 550) in y-up; Attached hangs 5 in and 5 down from it.
        assert_eq!(
            s.eval::<(f64, f64)>("return Attached:GetLeft(), Attached:GetTop()")
                .unwrap(),
            (105.0, 545.0),
            "a relativeTo-less anchor measures from the PARENT, not the screen"
        );

        assert!(s.eval::<bool>("return Orphan ~= nil").unwrap());
        assert!(s.eval::<bool>("return Orphan:GetParent() == nil").unwrap());
        assert!(
            report.warnings.iter().any(|w| w.contains("NoSuchFrame")),
            "the unresolvable parent is named in a warning: {:?}",
            report.warnings
        );

        // A global that is not a frame is the same miss, not a raise.
        s.run("NotAFrame = { some = 'table' }").unwrap();
        let doc2 = parse(
            r#"<Ui>
                <Frame name="Confused" parent="NotAFrame">
                    <Size><AbsDimension x="10" y="10"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                </Frame>
            </Ui>"#,
        );
        let report2 = load(&s, &doc2, &no_files);
        assert!(report2.errors.is_empty(), "{:?}", report2.errors);
        assert!(s.eval::<bool>("return Confused ~= nil").unwrap());
        assert!(s
            .eval::<bool>("return Confused:GetParent() == nil")
            .unwrap());
        assert!(
            report2.warnings.iter().any(|w| w.contains("NotAFrame")),
            "a non-frame global of the right name is named too: {:?}",
            report2.warnings
        );
    }

    /// A miss stores 0 over the default parent (`0x6ee3ef`), so the frame is built parentless; an
    /// empty `parent=""` keeps the default silently (`0x6ee3c7`).
    #[test]
    fn a_parent_attribute_that_misses_leaves_the_frame_parentless() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="Enclosing">
                    <Size><AbsDimension x="200" y="100"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                    <Frames>
                        <Frame name="$parentMissed" parent="NotLoadedYet">
                            <Size><AbsDimension x="10" y="10"/></Size>
                            <Anchors><Anchor point="CENTER"/></Anchors>
                        </Frame>
                        <Frame name="$parentEmpty" parent="">
                            <Size><AbsDimension x="10" y="10"/></Size>
                            <Anchors><Anchor point="CENTER"/></Anchors>
                        </Frame>
                    </Frames>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s.resolve();

        assert!(s.eval::<bool>("return TopMissed ~= nil").unwrap());
        assert!(
            s.eval::<bool>("return TopMissed:GetParent() == nil")
                .unwrap(),
            "a parent= that names nothing nulls the parent; it does not fall back"
        );
        // With no parent, `$parent` falls to the `"Top"` seed (`0x76c5b0`): `TopMissed`.
        assert!(s.eval::<bool>("return EnclosingMissed == nil").unwrap());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w == "Couldn't find frame parent: NotLoadedYet"),
            "logged in the reference's own words (0x8710f0): {:?}",
            report.warnings
        );

        assert_eq!(
            s.eval::<String>("return EnclosingEmpty:GetParent():GetName()")
                .unwrap(),
            "Enclosing",
            "parent=\"\" keeps the enclosing parent"
        );
        assert!(
            !report.warnings.iter().any(|w| w.contains("Empty")),
            "and says nothing about it: {:?}",
            report.warnings
        );
    }

    /// The reference attaches the parent (`0x6ee408`) before it applies `name=` (`0x6ee4d6`), and
    /// `$parent` walks the real parent chain (`0x76c5b0`); `parent=` itself is never expanded.
    #[test]
    fn a_dollar_parent_name_resolves_against_the_parent_attribute() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="PlayerFrame">
                    <Size><AbsDimension x="200" y="100"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                </Frame>
                <Frame name="$parentClassIcon" parent="PlayerFrame">
                    <Size><AbsDimension x="20" y="20"/></Size>
                    <Anchors><Anchor point="TOPRIGHT"/></Anchors>
                    <Frames>
                        <Frame name="$parentDot">
                            <Size><AbsDimension x="4" y="4"/></Size>
                            <Anchors><Anchor point="CENTER"/></Anchors>
                        </Frame>
                    </Frames>
                </Frame>
                <Frame name="$parentLoose">
                    <Size><AbsDimension x="10" y="10"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                </Frame>
                <Frame name="Literal" parent="$parentPlayerFrame">
                    <Size><AbsDimension x="10" y="10"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s.resolve();

        assert!(
            s.eval::<bool>("return PlayerFrameClassIcon ~= nil")
                .unwrap(),
            "the top-level $parent name took the parent= frame's name, not the lexical \"Top\""
        );
        assert!(s.eval::<bool>("return TopClassIcon == nil").unwrap());
        assert_eq!(
            s.eval::<String>("return PlayerFrameClassIcon:GetParent():GetName()")
                .unwrap(),
            "PlayerFrame",
            "and it is really attached there"
        );
        assert!(
            s.eval::<bool>("return PlayerFrameClassIconDot ~= nil")
                .unwrap(),
            "a nested child composes off the CORRECTED name"
        );

        // No attribute, no lexical parent: the `"Top"` seed.
        assert!(s.eval::<bool>("return TopLoose ~= nil").unwrap());

        // `parent=` is taken literally (`0x6ee3e8` hands it to `0x76c760`), so this one misses.
        assert!(s.eval::<bool>("return Literal:GetParent() == nil").unwrap());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("$parentPlayerFrame")),
            "the unexpanded attribute is named verbatim in the warning: {:?}",
            report.warnings
        );
    }

    /// Names, `$parent`, bottom-up `OnLoad` and the anchored texture rect, end to end.
    #[test]
    fn synthetic_full_document_materializes() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run("loadorder = {}").unwrap();

        let doc = parse(
            r#"<Ui>
                <Frame name="MyTemplate" virtual="true">
                    <Size><AbsDimension x="200" y="100"/></Size>
                </Frame>
                <Frame name="MyFrame" inherits="MyTemplate">
                    <Anchors>
                        <Anchor point="TOPLEFT" relativePoint="TOPLEFT">
                            <Offset><AbsDimension x="10" y="-20"/></Offset>
                        </Anchor>
                    </Anchors>
                    <Layers>
                        <Layer level="ARTWORK">
                            <Texture name="$parentTex">
                                <Color r="1" g="0" b="0" a="1"/>
                            </Texture>
                        </Layer>
                    </Layers>
                    <Frames>
                        <Frame name="$parentChild">
                            <Scripts>
                                <OnLoad>table.insert(loadorder, "child"); ChildLoaded = self:GetName()</OnLoad>
                            </Scripts>
                        </Frame>
                    </Frames>
                    <Scripts>
                        <OnLoad>table.insert(loadorder, "parent"); ParentLoaded = this:GetName()</OnLoad>
                    </Scripts>
                </Frame>
            </Ui>"#,
        );

        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert_eq!(report.frames, 2, "parent + child materialized");

        assert!(s.eval::<bool>("return MyFrame ~= nil").unwrap());
        assert!(s.eval::<bool>("return MyFrameChild ~= nil").unwrap());
        assert!(s.eval::<bool>("return MyFrameTex ~= nil").unwrap());

        // The child's handler uses `self`, the parent's `this`; both work.
        assert_eq!(
            s.eval::<String>("return ChildLoaded").unwrap(),
            "MyFrameChild"
        );
        assert_eq!(s.eval::<String>("return ParentLoaded").unwrap(), "MyFrame");

        // Bottom-up: the child's OnLoad ran before the parent's (`0x76a060`).
        let order: Vec<String> = s.eval("return loadorder").unwrap();
        assert_eq!(order, vec!["child".to_string(), "parent".to_string()]);

        // TOPLEFT+(10,-20), 200x100 on 800x600: bottom 480, left 10, top 580, right 210.
        s.resolve();
        let quads = s.extract();
        let tex = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { color: Some(_), .. }))
            .expect("the coloured texture quad");
        assert!(
            matches!(&tex.content, QuadContent::Texture { color: Some(c), path: None, .. } if *c == [1.0, 0.0, 0.0, 1.0])
        );
        assert_eq!(
            tex.rect,
            Some(crate::layout::Rect::new(480.0, 10.0, 580.0, 210.0))
        );
    }

    /// Expansion appends the instance's children after the template's and every `<Size>` applies
    /// in order, as the client processes each child, so the last wins.
    #[test]
    fn instance_size_overrides_template_size() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Button name="SizedTemplate" virtual="true">
                    <Size><AbsDimension x="80" y="22"/></Size>
                </Button>
                <Button name="SizedInstance" inherits="SizedTemplate">
                    <Size><AbsDimension x="125" y="21"/></Size>
                </Button>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert_eq!(
            s.eval::<f32>("return SizedInstance:GetWidth()").unwrap(),
            125.0
        );
        assert_eq!(
            s.eval::<f32>("return SizedInstance:GetHeight()").unwrap(),
            21.0
        );
    }

    #[test]
    fn frame_set_all_points_pins_to_parent() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="PinHost">
                    <Size><AbsDimension x="300" y="200"/></Size>
                    <Anchors>
                        <Anchor point="TOPLEFT">
                            <Offset><AbsDimension x="40" y="-50"/></Offset>
                        </Anchor>
                    </Anchors>
                    <Frames>
                        <Frame name="PinChild" setAllPoints="true"/>
                    </Frames>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();
        assert_eq!(s.eval::<f32>("return PinChild:GetLeft()").unwrap(), 40.0);
        assert_eq!(s.eval::<f32>("return PinChild:GetTop()").unwrap(), 550.0);
        assert_eq!(s.eval::<f32>("return PinChild:GetWidth()").unwrap(), 300.0);
        assert_eq!(s.eval::<f32>("return PinChild:GetHeight()").unwrap(), 200.0);
    }

    /// `<Include>` recurses into `0x6ede10`, which runs a path with a case-insensitive `.lua`
    /// suffix and parses anything else as XML; it never sniffs content.
    #[test]
    fn include_runs_a_lua_target_and_still_parses_an_xml_one() {
        let s = UiScript::new().unwrap();
        let provider = |path: &str| -> Option<Vec<u8>> {
            match path {
                "libs/Lib.lua" => Some(b"IncludedLua = 41 + 1".to_vec()),
                // Case-insensitive, exactly as `0x64a4c0` compares it.
                "libs/Shouty.LUA" => Some(b"ShoutyLua = true".to_vec()),
                "Sub.xml" => Some(br#"<Ui><Frame name="FromXmlInclude"/></Ui>"#.to_vec()),
                _ => None,
            }
        };
        let doc = parse(
            r#"<Ui>
                <Include file="libs\Lib.lua"/>
                <Include file="libs\Shouty.LUA"/>
                <Include file="Sub.xml"/>
                <Frame name="FromMain"/>
            </Ui>"#,
        );
        let report = load(&s, &doc, &provider);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            s.eval::<i64>("return IncludedLua").unwrap(),
            42,
            "a .lua Include must be executed as a chunk, not parsed as a document"
        );
        assert!(
            s.eval::<bool>("return ShoutyLua").unwrap(),
            "case-insensitive suffix"
        );
        assert_eq!(report.frames, 2);
        assert!(s
            .eval::<bool>("return FromXmlInclude ~= nil and FromMain ~= nil")
            .unwrap());
    }

    /// The reference logs the raise and falls through unconditionally (`0x6ee00d`-`0x6ee012`).
    #[test]
    fn a_raising_lua_include_does_not_take_the_rest_of_the_document() {
        let s = UiScript::new().unwrap();
        let provider = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Bad.lua" => Some(b"error('library exploded')".to_vec()),
                _ => None,
            }
        };
        let doc = parse(
            r#"<Ui>
                <Include file="Bad.lua"/>
                <Frame name="AfterTheBadInclude"/>
            </Ui>"#,
        );
        let report = load(&s, &doc, &provider);
        assert_eq!(
            report.errors.len(),
            1,
            "the raise is reported: {:?}",
            report.errors
        );
        assert!(report.errors[0].contains("Bad.lua"));
        assert!(
            s.eval::<bool>("return AfterTheBadInclude ~= nil").unwrap(),
            "the element after a failed Include must still be built"
        );
    }

    #[test]
    fn include_resolves_through_provider() {
        let s = UiScript::new().unwrap();
        let provider = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Sub.xml" => Some(br#"<Ui><Frame name="FromInclude"/></Ui>"#.to_vec()),
                _ => None,
            }
        };
        let doc = parse(
            r#"<Ui>
                <Include file="Sub.xml"/>
                <Frame name="FromMain"/>
            </Ui>"#,
        );
        let report = load(&s, &doc, &provider);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.frames, 2);
        assert!(s
            .eval::<bool>("return FromInclude ~= nil and FromMain ~= nil")
            .unwrap());
    }

    /// `CSimpleFrame::LoadXML 0x769820`: a strata `0x6f17d0` does not know logs at severity 1
    /// (`0x7699a4`) and skips `SetFrameStrata 0x76a470`.
    #[test]
    fn an_unknown_xml_frame_strata_warns_and_leaves_the_stratum_alone() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                 <Frame name="Bad" frameStrata="ARTWORK"/>
                 <Frame name="After"/>
               </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(
            report.errors.is_empty(),
            "nothing raised on the XML door: {:?}",
            report.errors
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("Unknown frame strata") && w.contains("ARTWORK")),
            "…and it is reported: {report:?}"
        );
        assert_eq!(
            s.eval::<String>("return Bad:GetFrameStrata()").unwrap(),
            "MEDIUM",
            "the frame keeps what it had — the ctor's MEDIUM ([+0xc0] = 3) in the base case"
        );
        assert!(
            s.eval::<bool>("return After ~= nil").unwrap(),
            "and the document carries on"
        );
    }

    /// The Lua `SetFrameStrata 0x774360` raises on the same value (`luaL_error` at `0x774456`).
    #[test]
    fn the_lua_setframestrata_still_raises_on_the_same_value() {
        let s = UiScript::new().unwrap();
        s.run(r#"f = CreateFrame("Frame", "Loud")"#).unwrap();
        let err = s
            .run(r#"f:SetFrameStrata("ARTWORK")"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ARTWORK"), "{err}");
    }

    /// The reference sets the flag from a number, `0` included, and only reads it for any other
    /// argument (`0x48845d`, after `lua_isnumber 0x6f34d0`).
    #[test]
    fn framexml_debug_is_a_get_or_set_and_gates_the_loader_traces() {
        let s = UiScript::new().unwrap();
        let doc = || parse(r#"<Ui><Frame name="Traced"/></Ui>"#);

        assert_eq!(
            s.eval::<i32>("return FrameXML_Debug()").unwrap(),
            0,
            "boots 0"
        );
        assert!(
            load(&s, &doc(), &no_files).traces.is_empty(),
            "and off means no trace at all"
        );

        assert_eq!(s.eval::<i32>("return FrameXML_Debug(1)").unwrap(), 1);
        let on = load(&s, &doc(), &no_files);
        assert!(
            on.traces
                .iter()
                .any(|t| t.contains("Creating Frame named Traced")),
            "{on:?}"
        );

        assert_eq!(
            s.eval::<i32>("return FrameXML_Debug(nil)").unwrap(),
            1,
            "nil is a pure GET — the flag is untouched"
        );
        assert_eq!(
            s.eval::<i32>("return FrameXML_Debug(0)").unwrap(),
            0,
            "…but 0 is TRUTHY in Lua, so it really does disable it"
        );
        assert!(load(&s, &doc(), &no_files).traces.is_empty());
        // The stored value truncates toward zero (`0x40a2b0`).
        assert_eq!(s.eval::<i32>("return FrameXML_Debug(1.9)").unwrap(), 1);
    }

    /// The reference logs `Couldn't open %s` (`0x6edaa0`, `0x846ff4`) and carries on.
    #[test]
    fn missing_include_is_a_missing_file_not_an_error_and_continues() {
        let s = UiScript::new().unwrap();
        let doc = parse(r#"<Ui><Include file="Nope.xml"/><Frame name="Still"/></Ui>"#);
        let report = load(&s, &doc, &no_files);
        assert!(
            report.missing_files.iter().any(|e| e.contains("Nope.xml")),
            "an unresolved include drops a whole document and says so: {report:?}"
        );
        assert!(
            report.errors.is_empty(),
            "…but nothing raised, so it is not a script error: {:?}",
            report.errors
        );
        assert!(
            s.eval::<bool>("return Still ~= nil").unwrap(),
            "and the load continues past it"
        );
    }

    /// The reference logs `"Error loading %s"` (`0x872e50`) for it and returns normally.
    #[test]
    fn missing_script_file_is_a_missing_file_not_an_error_and_continues() {
        let s = UiScript::new().unwrap();
        let doc = parse(r#"<Ui><Script file="Nope.lua"/><Frame name="Still"/></Ui>"#);
        let report = load(&s, &doc, &no_files);
        assert!(
            report.missing_files.iter().any(|e| e.contains("Nope.lua")),
            "{report:?}"
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(s.eval::<bool>("return Still ~= nil").unwrap());
    }

    #[test]
    fn a_script_file_that_raises_is_still_an_error() {
        let s = UiScript::new().unwrap();
        let doc = parse(r#"<Ui><Script file="Boom.lua"/><Frame name="Still"/></Ui>"#);
        let files = |req: &str| -> Option<Vec<u8>> {
            (req == "Boom.lua").then(|| b"error('boom')".to_vec())
        };
        let report = load(&s, &doc, &files);
        assert!(
            report.errors.iter().any(|e| e.contains("Boom.lua")),
            "a chunk that raised is an error, not a missing file: {report:?}"
        );
        assert!(report.missing_files.is_empty(), "{report:?}");
        assert!(s.eval::<bool>("return Still ~= nil").unwrap());
    }

    #[test]
    fn parent_token_in_child_name_and_anchor_relative_to() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="PF">
                    <Size><AbsDimension x="100" y="100"/></Size>
                    <Anchors>
                        <Anchor point="TOPLEFT"><Offset><AbsDimension x="50" y="-50"/></Offset></Anchor>
                    </Anchors>
                    <Frames>
                        <Frame name="$parentInner">
                            <Size><AbsDimension x="100" y="100"/></Size>
                            <Anchors>
                                <Anchor point="TOPLEFT" relativeTo="$parent" relativePoint="TOPLEFT"/>
                            </Anchors>
                        </Frame>
                    </Frames>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(s.eval::<bool>("return PFInner ~= nil").unwrap());

        s.resolve();
        let quads = s.extract();
        let rects: Vec<_> = quads
            .iter()
            .filter_map(|q| match q.target {
                ZTarget::Frame(_) => q.rect,
                _ => None,
            })
            .collect();
        // The parent is Rect(450,50,550,150); the child, pinned TOPLEFT to TOPLEFT, matches it.
        let expected = crate::layout::Rect::new(450.0, 50.0, 550.0, 150.0);
        assert!(
            rects.iter().filter(|r| **r == expected).count() >= 2,
            "rects: {rects:?}"
        );
    }

    /// The stock trainer row's shape: its label hangs off `$parentHighlight`'s RIGHT
    /// (`ClassTrainerFrameTemplates.xml:67`).
    #[test]
    fn a_named_state_texture_is_an_anchor_target_for_its_sibling_label() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Button name="Row">
                    <Size><AbsDimension x="293" y="16"/></Size>
                    <Anchors>
                        <Anchor point="TOPLEFT"><Offset><AbsDimension x="22" y="-50"/></Offset></Anchor>
                    </Anchors>
                    <HighlightTexture name="$parentHighlight" file="Interface\Buttons\UI-PlusButton-Hilight">
                        <Size><AbsDimension x="16" y="16"/></Size>
                        <Anchors>
                            <Anchor point="LEFT"><Offset><AbsDimension x="3" y="0"/></Offset></Anchor>
                        </Anchors>
                    </HighlightTexture>
                    <ButtonText name="$parentText">
                        <Size><AbsDimension x="0" y="13"/></Size>
                        <Anchors>
                            <Anchor point="LEFT" relativeTo="$parentHighlight" relativePoint="RIGHT">
                                <Offset><AbsDimension x="2" y="1"/></Offset>
                            </Anchor>
                        </Anchors>
                    </ButtonText>
                    <NormalFont inherits="GameFontNormal" justifyH="LEFT"/>
                </Button>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s.resolve();
        let (hl_right, text_left): (f64, f64) = s
            .eval("return RowHighlight:GetRight(), RowText:GetLeft()")
            .unwrap();
        assert_eq!(hl_right, 22.0 + 3.0 + 16.0);
        assert_eq!(
            text_left,
            hl_right + 2.0,
            "the label hangs off the highlight, not the button"
        );
        assert!(
            report
                .warnings
                .iter()
                .all(|w| !w.contains("does not resolve")),
            "no unresolved relativeTo: {:?}",
            report.warnings
        );
    }

    #[test]
    fn bad_handler_is_an_error_and_load_continues() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Frame name="Broken">
                    <Scripts><OnLoad>this that syntax error(((</OnLoad></Scripts>
                </Frame>
                <Frame name="Fine">
                    <Scripts><OnLoad>FineRan = true</OnLoad></Scripts>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert_eq!(report.frames, 2, "both frames still created");
        assert!(
            report.errors.iter().any(|e| e.contains("compiling")),
            "expected a compile error, got {:?}",
            report.errors
        );
        assert!(s.eval::<bool>("return Broken ~= nil").unwrap());
        assert!(s.eval::<bool>("return FineRan == true").unwrap());
    }

    /// `OnAttributeChanged` is 2.0's; no 1.12 handler resolver has a slot for it.
    #[test]
    fn unsupported_script_name_is_a_warning() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui><Frame name="Keyed"><Scripts><OnAttributeChanged>x = 1</OnAttributeChanged></Scripts></Frame></Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert_eq!(report.frames, 1);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("OnAttributeChanged")));
    }

    /// `Instantiate 0x6ee280` logs `"Unknown frame type: %s"` (`0x871124`) and builds nothing for
    /// the node or its `<Frames>`; only the Lua `CreateFrame` raises (`0x872fa8`).
    #[test]
    fn an_unknown_xml_frame_type_is_logged_and_its_node_skipped() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                 <Bogus name="X"><Frames><Frame name="Inside"/></Frames></Bogus>
                 <Frame name="Real"/>
               </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(
            report.errors.is_empty(),
            "nothing raised on the XML door: {:?}",
            report.errors
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w == "Unknown frame type: Bogus"),
            "the reference's own wording, in the log channel: {:?}",
            report.warnings
        );
        assert_eq!(report.frames, 1, "only the real frame built");
        assert!(s.eval::<bool>("return X == nil and Inside == nil").unwrap());
        assert!(s.eval::<bool>("return Real ~= nil").unwrap());
        // The Lua door raises on the same lookup.
        let err = s.run(r#"CreateFrame("Bogus")"#).unwrap_err().to_string();
        assert!(err.contains("unknown frame type 'Bogus'"), "{err}");
    }

    /// A `.toc` line hands a listed `Bindings.xml` to `0x6ede10` (from `0x6edd51`), so each
    /// `<Binding>` logs as an unknown frame type; `0x51f400` reads the bindings regardless.
    #[test]
    fn a_bindings_document_loaded_as_framexml_raises_nothing() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Bindings>
                 <Binding name="MONKEYDEV_STEPUP" header="MONKEYDEV">MonkeyStep_Inc()</Binding>
                 <Binding name="MONKEYDEV_STEPDOWN">MonkeyStep_Dec()</Binding>
               </Bindings>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|w| *w == "Unknown frame type: Binding")
                .count(),
            2,
            "one log line per <Binding>: {:?}",
            report.warnings
        );
        assert!(
            s.eval::<bool>("return MONKEYDEV_STEPUP == nil").unwrap(),
            "and no frame published under a binding's name"
        );
    }

    /// The WorldFrame type is one-shot, so a second `<WorldFrame>` is an unknown type.
    #[test]
    fn a_second_xml_world_frame_is_logged_not_raised() {
        let s = UiScript::new().unwrap();
        s.run(r#"CreateFrame("WorldFrame", "WorldFrame")"#).unwrap();
        let report = load(
            &s,
            &parse(r#"<Ui><WorldFrame name="Another"/></Ui>"#),
            &no_files,
        );
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(report
            .warnings
            .iter()
            .any(|w| w == "Unknown frame type: WorldFrame"));
        assert!(s.eval::<bool>("return Another == nil").unwrap());
    }

    /// `<StatusBar>` LoadXML extras (`0x782ef0`).
    #[test]
    fn statusbar_xml_extras_apply() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <StatusBar name="XmlBar" minValue="0" maxValue="100" defaultValue="50">
                    <Size><AbsDimension x="100" y="10"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT">
                            <Offset><AbsDimension x="0" y="0"/></Offset>
                        </Anchor>
                    </Anchors>
                    <BarTexture file="Interface\TargetingFrame\UI-StatusBar"/>
                    <BarColor r="0.1" g="0.9" b="0.1"/>
                </StatusBar>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        let ok: bool = s
            .eval(
                r#"
            local mn, mx = XmlBar:GetMinMaxValues()
            local r, g, b, a = XmlBar:GetStatusBarColor()
            return mn == 0 and mx == 100 and XmlBar:GetValue() == 50
                and XmlBar:GetOrientation() == "HORIZONTAL"
                and XmlBar:GetStatusBarTexture() ~= nil
                and r < 0.2 and g > 0.8 and a == 1
        "#,
            )
            .unwrap();
        assert!(ok, "XML attributes + children landed in the widget state");

        s.resolve();
        let bar = s
            .extract()
            .into_iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-StatusBar"))
            })
            .expect("bar fill quad");
        let r = bar.rect.expect("bar rect resolved");
        assert_eq!(
            (r.left, r.right, r.bottom, r.top),
            (0.0, 50.0, 0.0, 10.0),
            "defaultValue 50/100 fills half the 100px width"
        );
    }

    /// An anchorless `<Texture>` gets `SetAllPoints` after its LoadXML (`0x7701c0`), so its
    /// `<Size>` goes unread: `StackSplitFrame.xml:10` authors 256×32 and draws 172×96.
    #[test]
    fn sized_anchorless_layer_texture_fills_its_frame() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="Plate">
                    <Size><AbsDimension x="172" y="96"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="10" y="10"/></Offset></Anchor>
                    </Anchors>
                    <Layers><Layer level="BACKGROUND">
                        <Texture file="Interface\Panel">
                            <Size><AbsDimension x="256" y="32"/></Size>
                        </Texture>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();
        let plate = s
            .extract()
            .into_iter()
            .find(|q| {
                matches!(&q.content,
                    QuadContent::Texture { path: Some(p), .. } if p == "Interface\\Panel")
            })
            .expect("the plate draws");
        let r = plate.rect.expect("the plate resolved");
        assert_eq!(
            (r.left, r.right, r.bottom, r.top),
            (10.0, 182.0, 10.0, 106.0),
            "implicit SetAllPoints fills the 172×96 frame; the 256×32 size is unread"
        );
    }

    /// A FontString with no anchors gets one middle-row point from its justify word (`&7`: 1 LEFT,
    /// 4 RIGHT, else CENTER), so unlike a texture's its `<Size>` stays live.
    #[test]
    fn anchorless_fontstring_seats_at_its_justify_point() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="Page">
                    <Size><AbsDimension x="200" y="100"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="0" y="0"/></Offset></Anchor>
                    </Anchors>
                    <Layers><Layer level="ARTWORK">
                        <FontString name="SeatLeft" justifyH="LEFT" text="body">
                            <Size><AbsDimension x="80" y="20"/></Size>
                        </FontString>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();
        // LEFT to LEFT on the page's centreline, 80×20 from the size: [0,40]..[80,60].
        let (l, r, t, b) = s
            .eval::<(f64, f64, f64, f64)>(
                "return SeatLeft:GetLeft(), SeatLeft:GetRight(), SeatLeft:GetTop(), SeatLeft:GetBottom()",
            )
            .unwrap();
        assert_eq!(
            (l, r, b, t),
            (0.0, 80.0, 40.0, 60.0),
            "justifyH=LEFT seats the implicit single anchor at the page's LEFT, size live"
        );
    }

    /// The setter's implicit `SetAllPoints` is cleared before the authored anchors apply, so no
    /// implicit corner survives to weld the state texture to the button.
    #[test]
    fn anchored_state_texture_keeps_its_authored_anchors() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Button name="ScopedBtn">
                    <Size><AbsDimension x="36" y="36"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor>
                    </Anchors>
                    <NormalTexture file="Interface\Scoped">
                        <Size><AbsDimension x="24" y="24"/></Size>
                        <Anchors>
                            <Anchor point="TOPLEFT"><Offset><AbsDimension x="2" y="-2"/></Offset></Anchor>
                        </Anchors>
                    </NormalTexture>
                </Button>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();
        let scoped = s
            .extract()
            .into_iter()
            .find(|q| {
                matches!(&q.content,
                    QuadContent::Texture { path: Some(p), .. } if p == "Interface\\Scoped")
            })
            .expect("the state texture draws");
        let r = scoped.rect.expect("resolved");
        // Button [100,100]..[136,136]; TOPLEFT+2,-2 with 24×24 → [102,110]..[126,134].
        assert_eq!(
            (r.left, r.right, r.bottom, r.top),
            (102.0, 126.0, 110.0, 134.0),
            "authored anchors + size hold; no implicit corner welds it to the button"
        );
    }

    /// `0x7025fd` names a handler chunk `"%s:%s"` (`0x872a28`) from the object's `GetName`, or
    /// `<unnamed>` (`0x84c7f0`), and `0x704c70` loads the raw body, so body line n is chunk line n.
    #[test]
    fn an_xml_handler_chunk_is_named_by_its_frame_and_handler() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            "<Ui>\n\
             <Button name=\"NamedBtn\">\n\
             <Scripts>\n\
             <OnClick>\n\
             error(\"boom\")\n\
             </OnClick>\n\
             </Scripts>\n\
             </Button>\n\
             <Button>\n\
             <Scripts>\
             <OnLoad>BenillaAnon = this</OnLoad>\
             <OnClick>error(\"anon\")</OnClick>\
             </Scripts>\n\
             </Button>\n\
             <Button name=\"ProbeTmpl\" virtual=\"true\">\n\
             <Scripts><OnClick>error(\"tmpl\")</OnClick></Scripts>\n\
             </Button>\n\
             </Ui>",
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s.run(r#"Made = CreateFrame("Button", "Made", nil, "ProbeTmpl")"#)
            .unwrap();

        let raised = |lua: &str| -> String {
            s.eval::<String>(&format!("local ok, e = pcall({lua}) return tostring(e)"))
                .unwrap()
        };

        // The `error` is on the body's second line: `:2:`.
        let named = raised(r#"NamedBtn:GetScript("OnClick")"#);
        assert!(
            named.starts_with(r#"[string "NamedBtn:OnClick"]:2: boom"#),
            "{named}"
        );
        // A `CreateFrame` instance is named by its own `GetName()`, not by its template.
        let made = raised(r#"Made:GetScript("OnClick")"#);
        assert!(
            made.starts_with(r#"[string "Made:OnClick"]:1: tmpl"#),
            "{made}"
        );
        let anon = raised(r#"BenillaAnon:GetScript("OnClick")"#);
        assert!(
            anon.starts_with(r#"[string "<unnamed>:OnClick"]:1: anon"#),
            "{anon}"
        );
    }

    /// An inline `<Script>` is `"%s:<Scripts>"` (`0x871074`, at `0x6ee0ff`) over the document's
    /// path with no `@`, so it reads `[string "…"]`; a `<Script file=>` is `"@%s"` (`0x8716e0`).
    #[test]
    fn an_inline_script_chunk_is_named_for_the_document_not_as_a_file() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            "<Ui>\n<Script file=\"Sibling.lua\"/>\n<Script>\nBenillaInline = ({}).missing.deeper\n</Script>\n</Ui>",
        );
        let report = load_in(&s, &doc, "Interface/AddOns/Probe/Probe.xml", &|p: &str| {
            (p == "Interface/AddOns/Probe/Sibling.lua")
                .then(|| b"BenillaFile = ({}).missing.deeper".to_vec())
        });
        assert_eq!(report.errors.len(), 2, "{:?}", report.errors);
        assert!(
            report.errors[0].contains("Interface\\AddOns\\Probe\\Sibling.lua:1:"),
            "a <Script file=> chunk is still a plain `@`-path frame: {}",
            report.errors[0]
        );
        assert!(
            report.errors[1]
                .contains("[string \"Interface\\AddOns\\Probe\\Probe.xml:<Scripts>\"]:4:"),
            "an inline body is a `[string \"…\"]` frame naming the document: {}",
            report.errors[1]
        );
    }

    #[test]
    fn button_xml_extras_apply() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Button name="XmlBtn" text="Push Me">
                    <Size><AbsDimension x="64" y="24"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT">
                            <Offset><AbsDimension x="0" y="0"/></Offset>
                        </Anchor>
                    </Anchors>
                    <NormalTexture file="Interface\B-Up"/>
                    <PushedTexture file="Interface\B-Down"/>
                    <HighlightTexture file="Interface\B-Hi"/>
                    <Scripts>
                        <OnClick>xml_clicked = true</OnClick>
                    </Scripts>
                </Button>
                <CheckButton name="XmlCheck" checked="true">
                    <Size><AbsDimension x="24" y="24"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT">
                            <Offset><AbsDimension x="100" y="0"/></Offset>
                        </Anchor>
                    </Anchors>
                    <NormalTexture file="Interface\CB-Up"/>
                    <CheckedTexture file="Interface\CB-Check"/>
                </CheckButton>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        let ok: bool = s
            .eval(
                r#"
            return XmlBtn:GetText() == "Push Me"
               and XmlBtn:GetNormalTexture() ~= nil
               and XmlBtn:GetPushedTexture() ~= nil
               and XmlCheck:GetChecked() == 1
        "#,
            )
            .unwrap();
        assert!(ok, "button XML attrs + textures landed");

        s.resolve();
        let texs: Vec<String> = s
            .extract()
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert!(texs.contains(&"Interface\\B-Up".into()));
        assert!(!texs.contains(&"Interface\\B-Down".into()));
        assert!(!texs.contains(&"Interface\\B-Hi".into()));
        assert!(texs.contains(&"Interface\\CB-Check".into()));

        s.mouse_button(30.0, 10.0, "LeftButton", true);
        s.mouse_button(30.0, 10.0, "LeftButton", false);
        assert!(s.eval::<bool>("return xml_clicked == true").unwrap());
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn button_state_texture_takes_size_and_anchors() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Button name="ScopedBtn">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT">
                            <Offset><AbsDimension x="0" y="0"/></Offset>
                        </Anchor>
                    </Anchors>
                    <HighlightTexture file="Interface\Scoped-Hi" alphaMode="ADD">
                        <Size><AbsDimension x="20" y="20"/></Size>
                        <Anchors><Anchor point="TOPLEFT" relativePoint="TOPLEFT"/></Anchors>
                    </HighlightTexture>
                </Button>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        // Resolve first: hit-testing reads the resolved-rect cache.
        s.resolve();
        s.mouse_move(90.0, 15.0);
        s.resolve();
        let hi = s
            .extract()
            .into_iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("Scoped-Hi"))
            })
            .expect("hovered highlight quad");
        let r = hi.rect.expect("highlight rect resolved");
        assert_eq!(
            (r.left, r.right, r.bottom, r.top),
            (0.0, 20.0, 10.0, 30.0),
            "highlight is the anchored 20px square at the button's TOPLEFT, not the whole button"
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// `<EditBox>` LoadXML extras (`0x779fb0`).
    #[test]
    fn editbox_xml_extras_apply() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <EditBox name="XmlEdit" letters="5" numeric="true">
                    <Size><AbsDimension x="120" y="20"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT">
                            <Offset><AbsDimension x="0" y="0"/></Offset>
                        </Anchor>
                    </Anchors>
                    <Scripts>
                        <OnTextChanged>typed = true</OnTextChanged>
                    </Scripts>
                </EditBox>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        assert_eq!(s.eval::<i64>("return XmlEdit:GetNumLetters()").unwrap(), 0);
        s.run(r#"XmlEdit:Insert("12x")"#).unwrap(); // numeric: aborts
        s.run(r#"XmlEdit:Insert("123456")"#).unwrap(); // digits, capped at 5
        assert_eq!(
            s.eval::<String>("return XmlEdit:GetText()").unwrap(),
            "12345"
        );

        // A typed char marks the box changed; `OnTextChanged` fires on the next tick's drain.
        s.resolve();
        s.mouse_button(50.0, 10.0, "LeftButton", true);
        s.mouse_button(50.0, 10.0, "LeftButton", false);
        assert_eq!(s.focused_editbox_name().as_deref(), Some("XmlEdit"));
        s.run("typed = false").unwrap();
        assert!(s.char_input("7"));
        s.tick(0.0);
        assert!(s.eval::<bool>("return typed == true").unwrap());
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// The EditBox ctor leaves autoFocus on (`flags = 1`), so the stock UI writes the attribute
    /// only as `autoFocus="false"`: ten times across FrameXML and GlueXML.
    #[test]
    fn an_editbox_flag_written_false_in_xml_turns_the_flag_off() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <EditBox name="OptOut" autoFocus="false"/>
                <EditBox name="OptIn" autoFocus="true"/>
                <EditBox name="Silent"/>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        // Hidden first so `Show()` is a real transition: an autoFocus box takes focus on show.
        for (name, want) in [("OptOut", false), ("OptIn", true), ("Silent", true)] {
            s.run(&format!(
                "{name}:Hide(); {name}:ClearFocus(); {name}:Show()"
            ))
            .unwrap();
            assert_eq!(
                s.focused_editbox_name().as_deref() == Some(name),
                want,
                "{name}: autoFocus should be {want} (absent = the ctor default, ON)",
            );
            s.run(&format!("{name}:ClearFocus()")).unwrap();
        }
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    // ── TexCoords and font objects ──────────────────────────────────────────────────────────────

    /// The extracted quad carries the UV rect as `[left, right, top, bottom]`.
    #[test]
    fn texcoords_parse_to_uv_on_extracted_quad() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="TC">
                    <Size><AbsDimension x="100" y="100"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                    <Layers><Layer level="ARTWORK">
                        <Texture name="$parentArt" file="Interface\Foo">
                            <TexCoords left="0.0" right="0.5" top="0.25" bottom="0.75"/>
                        </Texture>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();
        let tex = s
            .extract()
            .into_iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
            .expect("the Foo texture quad");
        assert!(
            matches!(
                &tex.content,
                QuadContent::Texture { tex_coords: Some(crate::script::TexCoords::Rect(tc)), .. }
                    if *tc == [0.0, 0.5, 0.25, 0.75]
            ),
            "got {:?}",
            tex.content
        );
    }

    #[test]
    fn font_inherits_chain_resolves_height_and_color_two_levels() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Font name="Base" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="12"/></FontHeight>
                    <Color r="1.0" g="0.82" b="0"/>
                </Font>
                <Font name="Mid" inherits="Base" virtual="true">
                    <Color r="1.0" g="1.0" b="1.0"/>
                </Font>
                <Font name="Leaf" inherits="Mid" virtual="true">
                    <FontHeight><AbsValue val="10"/></FontHeight>
                </Font>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        let base = s.font_object("Base").expect("Base registered");
        assert_eq!(base.font.as_deref(), Some("Fonts\\FRIZQT__.TTF"));
        assert_eq!(base.height, Some(12.0));
        assert_eq!(base.color, Some([1.0, 0.82, 0.0, 1.0]));

        // Leaf: Base's face through Mid, Mid's white, its own height of 10.
        let leaf = s.font_object("Leaf").expect("Leaf registered");
        assert_eq!(leaf.font.as_deref(), Some("Fonts\\FRIZQT__.TTF"));
        assert_eq!(leaf.height, Some(10.0));
        assert_eq!(leaf.color, Some([1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn fontstring_inherits_reports_smaller_resolved_height() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="12"/></FontHeight>
                    <Color r="1.0" g="0.82" b="0"/>
                </Font>
                <Font name="GameFontNormalSmall" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="10"/></FontHeight>
                    <Color r="1.0" g="0.82" b="0"/>
                </Font>
                <Frame name="FS">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                    <Layers><Layer level="ARTWORK">
                        <FontString name="$parentBig" inherits="GameFontNormal" text="Big"/>
                        <FontString name="$parentSmall" inherits="GameFontNormalSmall" text="Small"/>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();

        let height_of = |s: &UiScript, want: &str| -> Option<f32> {
            s.extract().into_iter().find_map(|q| match q.content {
                QuadContent::Text {
                    text: Some(t),
                    font_height,
                    ..
                } if t == want => Some(font_height?),
                _ => None,
            })
        };
        assert_eq!(height_of(&s, "Big"), Some(12.0));
        assert_eq!(
            height_of(&s, "Small"),
            Some(10.0),
            "the *Small font object resolves to the smaller height"
        );

        let small = s
            .extract()
            .into_iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Small"))
            .unwrap();
        assert!(matches!(
            &small.content,
            QuadContent::Text { font: Some(f), color: Some(c), .. }
                if f == "Fonts\\FRIZQT__.TTF" && *c == [1.0, 0.82, 0.0, 1.0]
        ));
    }

    #[test]
    fn fontstring_color_overrides_object_and_unknown_inherits_warns() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="12"/></FontHeight>
                    <Color r="1.0" g="0.82" b="0"/>
                </Font>
                <Frame name="OV">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                    <Layers><Layer level="ARTWORK">
                        <FontString name="$parentA" inherits="GameFontNormal" text="A">
                            <Color r="0.1" g="0.2" b="0.3" a="1.0"/>
                        </FontString>
                        <FontString name="$parentB" inherits="NoSuchFont" text="B"/>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            report.warnings.iter().any(|w| w.contains("NoSuchFont")),
            "unknown font object should warn: {:?}",
            report.warnings
        );
        s.resolve();
        // A's own <Color> wins over the object's gold; its height still comes from the object.
        let a = s
            .extract()
            .into_iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "A"))
            .unwrap();
        assert!(matches!(
            &a.content,
            QuadContent::Text { color: Some(c), font_height: Some(h), .. }
                if *c == [0.1, 0.2, 0.3, 1.0] && *h == 12.0
        ));
        assert!(s
            .extract()
            .into_iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "B")));
    }

    /// The chat edit box's shape. The engine assigns the text slot from the direct-child
    /// `<FontString>` at LoadXML; it never searches the regions, so the header stays its own.
    #[test]
    fn editbox_adopts_special_fontstring_not_a_layers_header() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <EditBox name="EB" letters="255">
                    <Size><AbsDimension x="600" y="32"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT">
                            <Offset><AbsDimension x="100" y="100"/></Offset>
                        </Anchor>
                    </Anchors>
                    <TextInsets><AbsInset left="47" right="13" top="0" bottom="0"/></TextInsets>
                    <Layers>
                        <Layer level="ARTWORK">
                            <FontString name="$parentHeader" text="Say:">
                                <Anchors>
                                    <Anchor point="LEFT"><Offset><AbsDimension x="13" y="0"/></Offset></Anchor>
                                </Anchors>
                            </FontString>
                        </Layer>
                    </Layers>
                    <FontString justifyH="LEFT"/>
                </EditBox>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        s.run("EB:SetFocus()").unwrap();
        assert!(s.char_input("h"));
        assert!(s.char_input("i"));

        // Answer every text measure with a stand-in 30×12, then solve again.
        s.resolve();
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .iter()
            .map(|r| (r.id, 30.0, 12.0, r.key))
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.resolve();

        let quads = s.extract();
        let header = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Say:"))
            .expect("the header FontString still renders its own text");
        let typed = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "hi"))
            .expect("the typed text renders in the box's text region");

        // The header: its own LEFT+13 anchor and the 30×12 extent, centred on the 100..132 box.
        assert_eq!(
            header.rect,
            Some(crate::layout::Rect::new(110.0, 113.0, 122.0, 143.0)),
            "header must keep its own anchor + auto-size"
        );
        // Typed text rect: the box minus the XML `<TextInsets>` (47 left, 13 right).
        assert_eq!(
            typed.rect,
            Some(crate::layout::Rect::new(100.0, 147.0, 132.0, 687.0)),
            "the text region is anchored by the insets"
        );
    }

    /// Stock `TutorialFrame.xml:15`; drags like `CreateTitleRegion():SetAllPoints()` from Lua.
    #[test]
    fn title_region_element_builds_the_drag_handle_over_the_frame() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="Tut" enableMouse="true">
                    <Size><AbsDimension x="200" y="80"/></Size>
                    <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor></Anchors>
                    <TitleRegion setAllPoints="true"/>
                </Frame>
                <Frame name="Plain" enableMouse="true">
                    <Size><AbsDimension x="200" y="80"/></Size>
                    <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="400" y="100"/></Offset></Anchor></Anchors>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        let left = |s: &mut UiScript, f: &str| {
            s.resolve();
            s.eval::<f64>(&format!("return {f}:GetLeft()")).unwrap()
        };
        assert_eq!(left(&mut s, "Tut"), 100.0);
        s.mouse_button(150.0, 150.0, "LeftButton", true);
        s.mouse_move(250.0, 150.0);
        assert_eq!(
            left(&mut s, "Tut"),
            200.0,
            "the element's title region drags the frame"
        );
        s.mouse_button(250.0, 150.0, "LeftButton", false);

        assert_eq!(left(&mut s, "Plain"), 400.0);
        s.mouse_button(450.0, 150.0, "LeftButton", true);
        s.mouse_move(550.0, 150.0);
        assert_eq!(left(&mut s, "Plain"), 400.0, "no element, no handle");
        s.mouse_button(550.0, 150.0, "LeftButton", false);
    }

    #[test]
    fn a_loader_built_title_region_emits_no_quad() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Frame name="Tut" enableMouse="true">
                    <Size><AbsDimension x="200" y="80"/></Size>
                    <Anchors><Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="100" y="100"/></Offset></Anchor></Anchors>
                    <TitleRegion setAllPoints="true"/>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();
        let quads = s.extract();
        assert!(
            quads
                .iter()
                .all(|q| matches!(q.target, crate::order::ZTarget::Frame(_))),
            "a title region is a hit rectangle, never a quad: {quads:#?}"
        );
    }

    /// The stock micro-button: 29x58, `top="18"` insets (`MainMenuBarMicroButtons.xml:9`).
    #[test]
    fn hit_rect_insets_element_shrinks_the_mouse_rect() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Button name="Micro">
                    <Size><AbsDimension x="29" y="58"/></Size>
                    <Anchors><Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT"/></Anchors>
                    <HitRectInsets><AbsInset left="0" right="0" top="18" bottom="0"/></HitRectInsets>
                </Button>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        s.resolve();

        let (l, r, t, b) = s
            .eval::<(f64, f64, f64, f64)>("return Micro:GetHitRectInsets()")
            .unwrap();
        assert_eq!((l, r, t, b), (0.0, 0.0, 18.0, 0.0));
        assert_eq!(s.eval::<f64>("return Micro:GetHeight()").unwrap(), 58.0);
        // A button is mouse-enabled by construction, so the hit test is live.
        assert!(
            s.hit_test(14.0, 50.0).is_none(),
            "the dead 18-unit header must not capture"
        );
        assert!(s.hit_test(14.0, 20.0).is_some(), "the art band still hits");
    }

    /// The `<Scripts>` walker enables the mouse for `OnEnter`, `OnLeave`, `OnMouseDown`,
    /// `OnMouseUp` and `OnDragStart` (`0x769ef0` calls `0x76af00(2,-1)`), which is how the stock
    /// `GameTimeFrame` hovers; the Lua `SetScript` (`0x7748d0`) never does.
    #[test]
    fn scripts_block_mouse_handlers_auto_enable_mouse() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Frame name="Hoverable">
                    <Scripts><OnEnter>-- hover</OnEnter></Scripts>
                </Frame>
                <Frame name="DropTarget">
                    <Scripts><OnDragStop>-- outside the kind-2 set</OnDragStop></Scripts>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            s.eval::<bool>("return Hoverable:IsMouseEnabled()").unwrap(),
            "an XML mouse handler arms EnableMouse like the attribute would"
        );
        assert!(
            !s.eval::<bool>("return DropTarget:IsMouseEnabled()")
                .unwrap(),
            "OnDragStop is outside the kind-2 name set"
        );
        s.run("rt = CreateFrame('Frame', 'Rt'); rt:SetScript('OnEnter', function() end)")
            .unwrap();
        assert!(
            !s.eval::<bool>("return Rt:IsMouseEnabled()").unwrap(),
            "a runtime-created frame still needs an explicit EnableMouse"
        );
    }

    /// `CSimpleButton::LoadXML 0x7788c0` builds the label from `<NormalText>` (`0x778b6c`), clears
    /// its `ownsFontAttrs` (`0x778b7b`) and feeds the node to the Normal-state font (`0x783c30`,
    /// at `0x778baf`): the label takes the name and geometry, the state font the `inherits=`.
    #[test]
    fn a_button_normal_text_is_the_label_names_it_and_fonts_it() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Font name="KtmYellow" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="10"/></FontHeight>
                    <Color r="0.9" g="0.8" b="0.1" a="1"/>
                </Font>
                <Button name="KtmHeaderTemplate" virtual="true">
                    <NormalText name="$parentText" inherits="KtmYellow" text="Threat">
                        <Size><AbsDimension x="70" y="14"/></Size>
                        <Anchors><Anchor point="LEFT"/></Anchors>
                    </NormalText>
                </Button>
                <Frame name="KtmSelf">
                    <Size><AbsDimension x="300" y="100"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                    <Frames>
                        <Button name="$parentHeaderName" inherits="KtmHeaderTemplate">
                            <Size><AbsDimension x="70" y="14"/></Size>
                            <Anchors><Anchor point="TOPLEFT"/></Anchors>
                        </Button>
                    </Frames>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        // 1 · The global, `$parent` resolved against the instance.
        assert!(
            s.eval::<bool>("return KtmSelfHeaderNameText ~= nil")
                .unwrap(),
            "a named <NormalText> must publish its global"
        );
        // 2 · It is the button's label, not a loose FontString with the same name.
        assert_eq!(
            s.eval::<String>("return KtmSelfHeaderNameText:GetName()")
                .unwrap(),
            "KtmSelfHeaderNameText"
        );
        assert_eq!(
            s.eval::<String>("return KtmSelfHeaderName:GetFontString():GetName()")
                .unwrap(),
            "KtmSelfHeaderNameText",
            "the <NormalText> region must BE the button's label"
        );
        // 3 · The element's geometry landed on the label.
        assert_eq!(
            s.eval::<f64>("return KtmSelfHeaderNameText:GetWidth()")
                .unwrap(),
            70.0
        );
        // 4 · Its `inherits=` reached the Normal-state font, read off the painted quad because the
        // state font applies at extract.
        s.resolve();
        let label = s
            .extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text {
                    text: Some(t),
                    color,
                    ..
                } if t == "Threat" => Some(color),
                _ => None,
            })
            .expect("the <NormalText> label must paint");
        assert_eq!(
            label,
            Some([0.9, 0.8, 0.1, 1.0]),
            "<NormalText inherits=> must reach the button's Normal-state font"
        );
    }

    /// Only the `<NormalText>` leg clears the label's `+0x12c` gate (`0x778b7b`), so the label's
    /// `LoadXML 0x770f40` skips its font attributes, `justifyH` among them (`0x7710d3`), and
    /// `0x771480` seats the ctor's CENTER (`0x212`, `0x770dd3`). `<ButtonText>` goes through the
    /// ordinary FontString builder (`0x6f2780`) and seats LEFT. The justify still reaches the
    /// paint and `GetJustifyH()` through the state font (`0x783c30`), after the anchor is set.
    #[test]
    fn a_button_label_element_does_not_own_its_justify() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Font name="GatherFont" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="10"/></FontHeight>
                </Font>
                <Button name="GathererUI_PopupButtonTemplate" virtual="true">
                    <Size><AbsDimension x="64" y="12"/></Size>
                    <NormalText inherits="GatherFont" justifyH="LEFT"/>
                    <HighlightText inherits="GatherFont" justifyH="LEFT"/>
                    <DisabledText inherits="GatherFont" justifyH="LEFT"/>
                </Button>
                <Button name="OwnLabelTemplate" virtual="true">
                    <Size><AbsDimension x="64" y="12"/></Size>
                    <ButtonText inherits="GatherFont" justifyH="LEFT"/>
                </Button>
                <Frame name="GathererUI_Popup">
                    <Size><AbsDimension x="120" y="60"/></Size>
                    <Anchors>
                        <Anchor point="BOTTOMLEFT"><Offset><AbsDimension x="0" y="0"/></Offset></Anchor>
                    </Anchors>
                    <Frames>
                        <Button name="GathererUI_PopupButton1" inherits="GathererUI_PopupButtonTemplate" text="Minimap [on]">
                            <Anchors><Anchor point="TOP"/></Anchors>
                        </Button>
                        <Button name="OwnLabelButton" inherits="OwnLabelTemplate" text="Minimap [on]">
                            <Anchors><Anchor point="BOTTOM"/></Anchors>
                        </Button>
                    </Frames>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        // 1 · The label's anchor.
        let (point, rel_point, x, y) = s
            .eval::<(String, String, f64, f64)>(
                "local p, rel, rp, x, y = GathererUI_PopupButton1:GetFontString():GetPoint(1) \
                 return p, rp, x, y",
            )
            .unwrap();
        assert_eq!(
            (point.as_str(), rel_point.as_str(), x, y),
            ("CENTER", "CENTER", 0.0, 0.0),
            "<NormalText>'s justifyH is disowned, so 0x771480 seats the ctor's CENTER"
        );

        // 2 · The justify still reaches the label's getter through the state font.
        assert_eq!(
            s.eval::<String>("return GathererUI_PopupButton1:GetFontString():GetJustifyH()")
                .unwrap(),
            "LEFT",
            "the state font's justify still reaches the label"
        );

        // 3 · The control: `<ButtonText>` keeps its justify, so it seats LEFT.
        let (point, rel_point) = s
            .eval::<(String, String)>(
                "local p, rel, rp = OwnLabelButton:GetFontString():GetPoint(1) return p, rp",
            )
            .unwrap();
        assert_eq!(
            (point.as_str(), rel_point.as_str()),
            ("LEFT", "LEFT"),
            "<ButtonText> owns its font attributes — the gate is the <NormalText> leg's alone"
        );

        // 4 · Rows 64 wide centred on x=60 span [28,92]: a 40-wide label centred is [40,80],
        // seated LEFT [28,68].
        s.run(
            "GathererUI_PopupButton1:GetFontString():SetWidth(40) \
             OwnLabelButton:GetFontString():SetWidth(40)",
        )
        .unwrap();
        s.resolve();
        let (cl, cr, ll, lr) = s
            .eval::<(f64, f64, f64, f64)>(
                "local c = GathererUI_PopupButton1:GetFontString() \
                 local l = OwnLabelButton:GetFontString() \
                 return c:GetLeft(), c:GetRight(), l:GetLeft(), l:GetRight()",
            )
            .unwrap();
        assert_eq!(
            (cl, cr),
            (40.0, 80.0),
            "the Gatherer row's label is centred on the button, not hugging its left edge"
        );
        assert_eq!(
            (ll, lr),
            (28.0, 68.0),
            "the <ButtonText> row's label still hugs the button's left edge"
        );
    }

    /// `text=` resolves as a global (`FrameScript_GetText 0x703bf0`) on all three element shapes;
    /// a miss shows the raw value, as the reference's readers do, and a key-shaped miss warns.
    #[test]
    fn a_text_attribute_resolves_through_the_global_strings() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        // GlobalStrings.lua runs before any XML.
        s.run(r#"DELETE = "Delete" EXIT_GAME = "Exit Game" TITLE = "The Title""#)
            .unwrap();

        let doc = parse(
            r#"<Ui>
                <Frame name="Holder">
                    <Layers>
                        <Layer level="ARTWORK">
                            <FontString name="$parentTitle" text="TITLE"/>
                            <FontString name="$parentProse" text="No results found."/>
                        </Layer>
                    </Layers>
                    <Frames>
                        <Button name="$parentDelete" text="DELETE"/>
                        <Button name="$parentQuit">
                            <ButtonText name="$parentText" text="EXIT_GAME"/>
                        </Button>
                        <Button name="$parentGhost" text="NO_SUCH_KEY"/>
                    </Frames>
                </Frame>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        let texts: Vec<String> = s
            .eval(
                "return HolderTitle:GetText(), HolderProse:GetText(), HolderDelete:GetText(), \
                 HolderQuit:GetText(), HolderGhost:GetText()",
            )
            .map(|(a, b, c, d, e): (String, String, String, String, String)| vec![a, b, c, d, e])
            .unwrap();
        assert_eq!(
            texts,
            vec![
                "The Title",
                "No results found.",
                "Delete",
                "Exit Game",
                "NO_SUCH_KEY",
            ]
        );
        // Only the key-shaped miss warns.
        assert_eq!(
            report
                .warnings
                .iter()
                .filter(|w| w.contains("GlobalStrings key"))
                .count(),
            1,
            "warnings: {:?}",
            report.warnings
        );
        assert!(report.warnings.iter().any(|w| w.contains("NO_SUCH_KEY")));
    }

    #[test]
    fn key_shape_is_screaming_snake_of_two_or_more() {
        for yes in ["DELETE", "EXIT_GAME", "CHARACTER_POINTS1_COLON", "AB", "A1"] {
            assert!(is_global_string_key(yes), "{yes} is key-shaped");
        }
        for no in ["X", "", "Send Mail", "No results found.", "Okay", "1", "12"] {
            assert!(!is_global_string_key(no), "{no} is not key-shaped");
        }
    }

    // ── <SimpleHTML> ─────────────────────────────────────────────────────────────────────────

    /// A direct-child `<FontString>` declares `elementFont[0]` and `<FontStringHeader1>`
    /// `elementFont[1]`, neither a region, and `hyperlinkFormat=` lands at `+0x360`; stock
    /// `ItemTextFrame.xml:214` uses the first.
    #[test]
    fn simplehtml_xml_declares_element_fonts_not_regions() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Font name="ItemTextFontNormal" font="Fonts\MORPHEUS.TTF" justifyH="LEFT">
                    <FontHeight val="15"/>
                    <Color r="0.18" g="0.12" b="0.06"/>
                </Font>
                <Font name="BookHeader" font="Fonts\SKURRI.TTF">
                    <FontHeight val="24"/>
                </Font>
                <SimpleHTML name="PageText" hyperlinkFormat="|H%s|h[%s]|h">
                    <Size><AbsDimension x="270" y="304"/></Size>
                    <Anchors><Anchor point="TOPLEFT"/></Anchors>
                    <FontString inherits="ItemTextFontNormal"/>
                    <FontStringHeader1 inherits="BookHeader"/>
                    <Scripts><OnLoad>LOADED = this:GetName()</OnLoad></Scripts>
                </SimpleHTML>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        assert_eq!(s.eval::<String>("return LOADED").unwrap(), "PageText");
        assert_eq!(
            s.eval::<String>("return PageText:GetObjectType()").unwrap(),
            "SimpleHTML"
        );
        assert_eq!(
            s.eval::<String>("return PageText:GetHyperlinkFormat()")
                .unwrap(),
            "|H%s|h[%s]|h"
        );
        assert_eq!(
            s.eval::<(String, f32)>("local p, h = PageText:GetFont(); return p, h")
                .unwrap(),
            ("Fonts\\MORPHEUS.TTF".to_string(), 15.0)
        );
        assert_eq!(
            s.eval::<(String, f32)>("local p, h = PageText:GetFont(\"H1\"); return p, h")
                .unwrap(),
            ("Fonts\\SKURRI.TTF".to_string(), 24.0)
        );
        // Neither declaration became a region.
        let regions = {
            let lua = s.lua();
            let model = lua
                .app_data_ref::<crate::script::Model>()
                .expect("model app_data");
            let fh = model.arena.lookup("PageText").expect("frame");
            model.arena.frame(fh).expect("live").regions.len()
        };
        assert_eq!(regions, 0, "the font declarations created no regions");

        // Two blocks, the H1 in its declared face, at the frame's width.
        s.run(
            r#"PageText:SetText("<HTML><BODY><H1>Title</H1><P align=\"center\">Body</P></BODY></HTML>")"#,
        )
        .unwrap();
        let (kinds, fonts, widths) = {
            let lua = s.lua();
            let model = lua
                .app_data_ref::<crate::script::Model>()
                .expect("model app_data");
            let fh = model.arena.lookup("PageText").expect("frame");
            let blocks = &model.simple_html.get(&fh).expect("state").blocks;
            (
                blocks.len(),
                blocks
                    .iter()
                    .map(|rh| model.region_data[rh].font_path.clone())
                    .collect::<Vec<_>>(),
                blocks
                    .iter()
                    .map(|rh| model.region_data[rh].size)
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(kinds, 2);
        assert_eq!(
            fonts,
            [
                Some("Fonts\\SKURRI.TTF".to_string()),
                Some("Fonts\\MORPHEUS.TTF".to_string())
            ],
            "a DECLARED header font wins; the P falls through to its own"
        );
        assert_eq!(widths, [Some((270.0, 0.0)); 2]);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// `CSimpleButton::LoadXML 0x7788c0` builds any `<...Texture>` child through the `<Layers>`
    /// texture adder (`0x6f26f0`); `SpellBookFrame.xml:36` writes a bare `<NormalTexture/>`.
    #[test]
    fn a_bare_state_texture_element_still_builds_its_region() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        let doc = parse(
            r#"<Ui>
                <Button name="BareBtn">
                    <Size><AbsDimension x="32" y="32"/></Size>
                    <Anchors><Anchor point="CENTER"/></Anchors>
                    <NormalTexture/>
                    <DisabledTexture />
                </Button>
            </Ui>"#,
        );
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);

        assert!(
            s.eval::<bool>("return BareBtn:GetNormalTexture() ~= nil")
                .unwrap(),
            "a bare <NormalTexture/> builds its region"
        );
        assert!(
            s.eval::<bool>("return BareBtn:GetDisabledTexture() ~= nil")
                .unwrap(),
            "a bare <DisabledTexture /> builds its region"
        );
        // Live enough for `SetTexCoord`, with no art of its own.
        s.run("BareBtn:GetNormalTexture():SetTexCoord(0.07, 0.93, 0.07, 0.93)")
            .unwrap();
        assert!(
            s.eval::<bool>("return BareBtn:GetNormalTexture():GetTexture() == nil")
                .unwrap(),
            "blank, not painted"
        );
    }
}

mod layer_blend_tests {
    use crate::framexml::parse;
    use crate::loader::*;
    use crate::script::{QuadContent, UiScript};

    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn layers_texture_alpha_mode_add_reaches_the_quad() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Frame name="GlowHost">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors><Anchor point="BOTTOMLEFT" relativePoint="BOTTOMLEFT"/></Anchors>
                    <Layers><Layer level="OVERLAY">
                        <Texture name="GlowTex" file="Interface\Glow" alphaMode="ADD">
                            <Anchors><Anchor point="TOPLEFT"/><Anchor point="BOTTOMRIGHT"/></Anchors>
                        </Texture>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        )
        .unwrap();
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        let mut s = s;
        s.set_screen_size(800.0, 600.0);
        s.resolve();
        let additive = s
            .extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(p),
                    additive,
                    ..
                } if p == "Interface\\Glow" => Some(*additive),
                _ => None,
            })
            .expect("the layer texture extracted");
        assert!(additive, "alphaMode=ADD must ride into the quad blend flag");
    }
}

mod region_template_tests {
    use crate::framexml::parse;
    use crate::loader::*;
    use crate::script::{QuadContent, UiScript};

    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    /// The instance's own nodes win over the template's.
    #[test]
    fn layers_texture_inherits_a_virtual_texture_template() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Texture name="BranchTemplate" file="Interface\Branches" virtual="true">
                    <Size><AbsDimension x="32" y="32"/></Size>
                    <Anchors><Anchor point="TOPLEFT"/></Anchors>
                </Texture>
                <Frame name="Host">
                    <Size><AbsDimension x="300" y="300"/></Size>
                    <Anchors><Anchor point="TOPLEFT" relativePoint="TOPLEFT"/></Anchors>
                    <Layers><Layer level="BACKGROUND">
                        <Texture name="Branch1" inherits="BranchTemplate"/>
                        <Texture name="Branch2" inherits="BranchTemplate">
                            <Size><AbsDimension x="64" y="16"/></Size>
                        </Texture>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        )
        .unwrap();
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            report.warnings.is_empty(),
            "a registered region template must splice silently: {:?}",
            report.warnings
        );
        let mut s = s;
        s.set_screen_size(800.0, 600.0);
        assert_eq!(
            s.eval::<(f64, f64)>("return Branch1:GetWidth(), Branch1:GetHeight()")
                .unwrap(),
            (32.0, 32.0)
        );
        assert_eq!(
            s.eval::<(f64, f64)>("return Branch2:GetWidth(), Branch2:GetHeight()")
                .unwrap(),
            (64.0, 16.0)
        );
        s.resolve();
        let branch_quads = s
            .extract()
            .iter()
            .filter(|q| {
                matches!(
                    &q.content,
                    QuadContent::Texture { path: Some(p), .. } if p == "Interface\\Branches"
                )
            })
            .count();
        assert_eq!(
            branch_quads, 2,
            "both templated textures extract with the template's file"
        );
    }

    #[test]
    fn fontstring_font_object_inherits_stays_out_of_the_template_path() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Font name="GameFontNormal" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="12"/></FontHeight>
                </Font>
                <Frame name="Host2">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors><Anchor point="TOPLEFT" relativePoint="TOPLEFT"/></Anchors>
                    <Layers><Layer level="ARTWORK">
                        <FontString name="Label" inherits="GameFontNormal" text="hello"/>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        )
        .unwrap();
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            report.warnings.is_empty(),
            "a font-object inherits must not warn as an unknown template: {:?}",
            report.warnings
        );
        assert_eq!(s.eval::<String>("return Label:GetText()").unwrap(), "hello");
        assert_eq!(
            s.eval::<(String, f32)>("local f, h = Label:GetFont() return f, h")
                .unwrap(),
            ("Fonts\\FRIZQT__.TTF".to_string(), 12.0),
            "a FontString that inherits a Font object reports that object's font"
        );
    }

    /// The reference's font registry compares names with `SStrCmpI` (`0x783870`/`0x7838c7`).
    #[test]
    fn a_font_object_inherits_resolves_whatever_its_case() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Font name="GameFontHighlightSmall" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="10"/></FontHeight>
                </Font>
                <Frame name="CaseHost">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors><Anchor point="TOPLEFT" relativePoint="TOPLEFT"/></Anchors>
                    <Layers><Layer level="ARTWORK">
                        <FontString name="CaseLabel" inherits="GameFontHighLightSmall" text="hi"/>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        )
        .unwrap();
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(
            report.warnings.is_empty(),
            "a differently-cased font name must resolve, not warn: {:?}",
            report.warnings
        );
        assert_eq!(
            s.eval::<(String, f32)>("local f, h = CaseLabel:GetFont() return f, h")
                .unwrap(),
            ("Fonts\\FRIZQT__.TTF".to_string(), 10.0),
            "the mis-cased inherit found the shipped font"
        );
    }

    /// Two hops: a FontString inherits a virtual FontString template, which inherits a font object.
    #[test]
    fn a_fontstring_template_carries_the_font_object_it_inherits() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Font name="GameFontHighlight" font="Fonts\FRIZQT__.TTF" virtual="true">
                    <FontHeight><AbsValue val="12"/></FontHeight>
                </Font>
                <FontString name="LineTemplate" inherits="GameFontHighlight" virtual="true"
                            justifyH="LEFT"/>
                <Frame name="Host3">
                    <Size><AbsDimension x="100" y="30"/></Size>
                    <Anchors><Anchor point="TOPLEFT" relativePoint="TOPLEFT"/></Anchors>
                    <Layers><Layer level="ARTWORK">
                        <FontString name="Line1" inherits="LineTemplate" text="one"/>
                    </Layer></Layers>
                </Frame>
            </Ui>"#,
        )
        .unwrap();
        let report = load(&s, &doc, &no_files);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert_eq!(s.eval::<String>("return Line1:GetText()").unwrap(), "one");
        assert_eq!(
            s.eval::<(String, f32)>("local f, h = Line1:GetFont() return f, h")
                .unwrap(),
            ("Fonts\\FRIZQT__.TTF".to_string(), 12.0),
            "the font survives BOTH hops — template inheritance must not drop it"
        );
    }
}

/// Chunk naming: the file and line a `<Script>` raise reports.
mod chunk_name_tests {
    use crate::framexml;
    use crate::loader::*;
    use crate::script::UiScript;

    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    fn parse(text: &str) -> framexml::ParsedDocument {
        framexml::parse(text).expect("valid FrameXML")
    }

    /// An inline block is padded to its line in the file, so a raise reports the file's line.
    #[test]
    fn a_script_raise_names_its_own_file_and_line() {
        let s = UiScript::new().unwrap();
        // `error()` with no level prefixes `chunkname:line:`, as a real raise does.
        let doc = parse(
            "<Ui>\n\
             <Frame name=\"Pad\"/>\n\
             <Script>\n\
             local x\n\
             error(\"boom\")\n\
             </Script>\n\
             </Ui>",
        );
        let report = load_in(&s, &doc, "Bagnon/src/main.xml", &no_files);
        let err = report.errors.join("\n");
        // Backslashes, and the reference's `"%s:<Scripts>"` form (`0x871074`).
        assert!(
            err.contains("Bagnon\\src\\main.xml:"),
            "the raise must name the document, got: {err}"
        );
        // `error("boom")` is line 5 of the literal; unpadded it would read `:2:`.
        assert!(
            err.contains("main.xml:<Scripts>\"]:5:"),
            "the line must be the FILE's line, not the block's, got: {err}"
        );
    }

    #[test]
    fn an_included_documents_script_names_the_included_file() {
        let s = UiScript::new().unwrap();
        let inner = "<Ui>\n<Script>\nerror(\"inner\")\n</Script>\n</Ui>";
        let files = |req: &str| -> Option<Vec<u8>> {
            (req == "Addon/sub/inner.xml").then(|| inner.as_bytes().to_vec())
        };
        let doc = parse("<Ui>\n<Include file=\"sub\\inner.xml\"/>\n</Ui>");
        let report = load_in(&s, &doc, "Addon/outer.xml", &files);
        let err = report.errors.join("\n");
        assert!(
            err.contains("Addon\\sub\\inner.xml:<Scripts>\"]:3:"),
            "the INCLUDED file and its line, not the includer's: {err}"
        );
        assert!(
            !err.contains("outer.xml"),
            "the includer must not be blamed: {err}"
        );
    }

    /// The line comes from the text child's own range, not the element's, which the `<![CDATA[`
    /// opener would offset.
    #[test]
    fn a_cdata_script_block_reports_the_files_line() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            "<Ui>\n\
             <Frame name=\"A\"/>\n\
             <Frame name=\"B\"/>\n\
             <Script><![CDATA[\n\
             local ok = 1\n\
             error(\"cdata boom\")\n\
             ]]></Script>\n\
             </Ui>",
        );
        let report = load_in(&s, &doc, "Ours/Bag.xml", &no_files);
        let err = report.errors.join("\n");
        assert!(
            err.contains("Ours\\Bag.xml:<Scripts>\"]:6:"),
            "line 6 is `error(\"cdata boom\")` in the literal above, got: {err}"
        );
    }

    #[test]
    fn a_script_file_chunk_is_named_after_that_file() {
        let s = UiScript::new().unwrap();
        let files = |req: &str| -> Option<Vec<u8>> {
            (req == "Addon/code.lua").then(|| b"\nerror(\"from lua\")\n".to_vec())
        };
        let doc = parse("<Ui>\n<Script file=\"code.lua\"/>\n</Ui>");
        let report = load_in(&s, &doc, "Addon/host.xml", &files);
        let err = report.errors.join("\n");
        assert!(
            err.contains("Addon\\code.lua:2:"),
            "the .lua file and its own line: {err}"
        );
    }

    /// The 1.12 hook idiom: an addon captures a handler with `GetScript` and calls it with no
    /// arguments, the frame passed only as `this`; `Loader::compile_handler` falls back to `this`.
    #[test]
    fn a_script_captured_and_called_with_no_arguments_still_sees_its_frame() {
        let s = UiScript::new().unwrap();
        let doc = parse(
            r#"<Ui>
                <Frame name="Hooked" hidden="true">
                    <Scripts>
                        <OnShow>SEEN = self:GetName()</OnShow>
                    </Scripts>
                </Frame>
            </Ui>"#,
        );
        let report = load_in(&s, &doc, "Test.xml", &no_files);
        assert!(report.errors.is_empty(), "{:?}", report.errors);

        // The engine's call passes `self` and sets `this`.
        s.run("SEEN = nil Hooked:Show()").unwrap();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "Hooked");

        // The addon's: the captured script called bare, with only `this` set.
        s.run(
            "SEEN = nil \
             local original = Hooked:GetScript(\"OnShow\") \
             this = Hooked \
             original() \
             this = nil",
        )
        .unwrap();
        assert_eq!(
            s.eval::<String>("return SEEN").unwrap(),
            "Hooked",
            "the reference's own no-argument contract must reach the same frame"
        );
    }
}

#[test]
fn join_ref_resolves_relative_framexml_paths() {
    use super::join_ref;
    assert_eq!(
        join_ref("Bagnon/src", "templates.xml"),
        "Bagnon/src/templates.xml"
    );
    assert_eq!(
        join_ref("Bagnon/src", "..\\..\\BagBrother\\core\\core.xml"),
        "BagBrother/core/core.xml"
    );
    assert_eq!(join_ref("", "Fonts.xml"), "Fonts.xml");
    assert_eq!(join_ref("a/b", "./c//d.xml"), "a/b/c/d.xml");
    // An escape above the root survives, so the provider can refuse it.
    assert_eq!(join_ref("a", "../../secret"), "../secret");
    assert_eq!(join_ref("", "../secret"), "../secret");
    // A leading `/` re-roots rather than meaning the filesystem root.
    assert_eq!(join_ref("a/b", "/c.xml"), "c.xml");
}
