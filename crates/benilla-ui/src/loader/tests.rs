//! Loader tests — split from `mod.rs` (file-size budget); same `super::*` view.

mod loader_tests {
    use crate::framexml;
    use crate::loader::*;
    use crate::order::ZTarget;
    use crate::script::{QuadContent, UiScript};

    /// No-provider `files` closure (for docs with no `<Include>`/`<Script file=>`).
    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    fn parse(text: &str) -> framexml::ParsedDocument {
        framexml::parse(text).expect("valid FrameXML")
    }

    /// **`<Minimap>`'s two model attributes name the nine engine children** — the model half of
    /// `CMinimap::LoadXML 0x4ee2b0`. `minimapArrowModel` goes to children 1–8 through `0x4ee170`'s
    /// two loops (the five at `[+0x320]`, the three POI arrows at `[+0x314]`);
    /// `minimapPlayerModel` goes to `[Minimap+0x338]` — child **9** — alone, through `0x4ee260`.
    /// Without the split all nine read back `GetModel() == ""` and nothing in the tuple says which
    /// one is the player arrow.
    ///
    /// Both halves in one document: the shipped spelling (`Minimap.xml:97` sets both explicitly)
    /// and an attribute-less `<Minimap>`, which takes the engine defaults `0x84c768` / `0x8453c0`
    /// rather than staying file-less.
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

        // 1–8 wear the arrow, 9 wears the player model — the authored spelling, unchanged.
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

        // The `<Frames>` child lands PAST the nine, because the factory runs before the descent
        // (`0x6ee408` before `0x6ee4ea`) and both linkers append at the tail. This is the whole
        // reason `[9]` is a stable address for an addon.
        assert_eq!(
            s.eval::<String>("return ({Minimap:GetChildren()})[10]:GetName()")
                .unwrap(),
            "MiniMapPingHost"
        );

        // No attributes: the engine defaults, not nothing.
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

    /// **`parent="Name"` attaches a top-level element** — the reference's own way of doing it,
    /// because its FrameXML is flat (decision 1211). 79 corpus addons, 708 sites.
    ///
    /// Three claims in one document: the attribute supplies the parent when there is no lexical
    /// one; an anchor with no `relativeTo` then measures from THAT parent rather than the screen
    /// (the silent half — a frame in the wrong place with nothing to report); and a name that
    /// resolves to nothing warns and falls back instead of raising.
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

        // The miss: a warning, a usable frame, and no parent — never an error.
        assert!(s.eval::<bool>("return Orphan ~= nil").unwrap());
        assert!(s.eval::<bool>("return Orphan:GetParent() == nil").unwrap());
        assert!(
            report.warnings.iter().any(|w| w.contains("NoSuchFrame")),
            "the unresolvable parent is named in a warning: {:?}",
            report.warnings
        );

        // And a name that exists but is NOT a frame is the same miss, not a raise. `_G` is one
        // namespace and an addon's own `MyAddon = {}` lives in it — the corpus writes
        // `parent="TheoryCraft"` where the addon owns that name. Handing a plain table to
        // CreateFrame would kill the element and its whole subtree.
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

    /// **A `parent=` that resolves to nothing leaves the frame PARENTLESS** — it does not fall back
    /// to the enclosing frame (decision 2213). `0x6ee280` seeds `[ebp-0x8]` with the incoming
    /// default parent and then writes the lookup's result back **unconditionally** at
    /// `0x6ee3ef`, so a miss stores 0 over the seed and `0x6ee408` constructs with `ecx = 0`. And
    /// an **empty** `parent=""` short-circuits
    /// at `0x6ee3c7` before any of that: the default parent is kept, silently.
    ///
    /// Only the nested case can tell these apart — at top level there is no enclosing frame to
    /// fall back to, which is why
    /// [`Self::a_top_level_parent_attribute_attaches_and_anchors`] passed either way.
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

        // The miss: parentless, NOT re-attached to Enclosing.
        assert!(s.eval::<bool>("return TopMissed ~= nil").unwrap());
        assert!(
            s.eval::<bool>("return TopMissed:GetParent() == nil")
                .unwrap(),
            "a parent= that names nothing nulls the parent; it does not fall back"
        );
        // …and with no parent, the `$parent` chain walk finds nothing, so the "Top" seed survives
        // (`0x76c5b0`) — hence `TopMissed`, not `EnclosingMissed`.
        assert!(s.eval::<bool>("return EnclosingMissed == nil").unwrap());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w == "Couldn't find frame parent: NotLoadedYet"),
            "logged in the reference's own words (0x8710f0): {:?}",
            report.warnings
        );

        // The empty attribute: the enclosing frame is kept, and nothing is logged.
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

    /// **A top-level `name="$parent…"` resolves against the `parent=` attribute** (B387) — the
    /// reference attaches the parent *first* (`0x6ee280` resolves it at `0x6ee3e8` and passes it to
    /// the constructor at `0x6ee408`) and only then runs the node-apply step that reads `name=` and
    /// calls `SetName` (`0x6ee4d6`), whose expander walks the frame's **actual** parent chain
    /// (`0x76c5b0`). We resolved the name first, against the lexical ancestor, so ClassIcons'
    /// `<Frame name="$parentClassIcon" parent="PlayerFrame"/>` was published as `TopClassIcon` and
    /// every `PlayerFrameClassIcon` lookup in the addon indexed a nil.
    ///
    /// Four claims: the name takes the attribute parent's name; that frame's own children compose
    /// off the corrected name; a top-level `$parent` with no attribute still falls to the `"Top"`
    /// seed; and the `parent=` attribute itself is NOT expanded (`0x6ee3e8` calls the by-name
    /// resolver direct, bypassing `0x76c5b0`).
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

        // No attribute, no lexical parent: the expander's `"Top"` seed survives (`0x76c5b0`).
        assert!(s.eval::<bool>("return TopLoose ~= nil").unwrap());

        // `parent="$parentPlayerFrame"` is taken LITERALLY — `0x6ee3e8` hands the raw string to
        // `0x76c760`, so no frame of that name exists and the element falls back (and warns).
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

    /// End-to-end: a virtual template, an instance inheriting it (with `<Size>`, screen `<Anchors>`,
    /// a `<Layers>` coloured `<Texture>`, a nested child `<Frame>` in `<Frames>`, and `<OnLoad>`
    /// handlers on both) — proving name publication, bottom-up OnLoad, `$parent`, and that
    /// resolve()+extract() places the texture at the anchored rect.
    #[test]
    fn synthetic_full_document_materializes() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        // Seed the ordering witness before the load runs any OnLoad.
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

        // Frame exists by name; the child's $parent resolved to MyFrameChild; the texture published
        // its $parent-resolved name.
        assert!(s.eval::<bool>("return MyFrame ~= nil").unwrap());
        assert!(s.eval::<bool>("return MyFrameChild ~= nil").unwrap());
        assert!(s.eval::<bool>("return MyFrameTex ~= nil").unwrap());

        // OnLoad fired for both (modern `self` in child, legacy `this` in parent — both work).
        assert_eq!(
            s.eval::<String>("return ChildLoaded").unwrap(),
            "MyFrameChild"
        );
        assert_eq!(s.eval::<String>("return ParentLoaded").unwrap(), "MyFrame");

        // Bottom-up: the child's OnLoad ran before the parent's (`0x76a060`).
        let order: Vec<String> = s.eval("return loadorder").unwrap();
        assert_eq!(order, vec!["child".to_string(), "parent".to_string()]);

        // resolve()+extract(): the texture quad sits at MyFrame's anchored rect.
        // Screen [0,0,600,800], TOPLEFT+(10,-20), size 200x100 → Rect(bottom 480, left 10, top 580,
        // right 210). Region rect = its owner frame's resolved rect (v1).
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

    /// An instance's own `<Size>` beats its template's: expansion appends the instance's children
    /// after the template's, and `apply_size` walks EVERY `<Size>` in document order so the last
    /// write wins — exactly the client's process-each-child behavior. (Regression: `.next()` here
    /// once pinned every templated button to the TEMPLATE's size; the quest log's Abandon button
    /// declared 125×21 and rendered 80×22, wrapping its label onto two lines.)
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

    /// A `<Frame setAllPoints="true">` (no `<Size>`/`<Anchors>` of its own) resolves to its
    /// parent's rect — the frame path honors the shorthand like the region path does.
    /// (Regression: only regions applied it; the world map's chrome frames carried the attribute,
    /// never resolved, and their whole subtrees — backdrop, title, buttons — silently vanished.)
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

    /// **`<Include file="X.lua">` RUNS the file** — the client's `<Include>` is a recursion into
    /// its load-one-file routine `0x6ede10`, whose first act is a case-insensitive `.lua` suffix
    /// test on the resolved path. It never sniffs content; it never parses a `.lua` as XML.
    ///
    /// We parsed every target as FrameXML, so a `.lua` target died `unknown token at 1:1` and took
    /// the whole library set with it. Three corpus addons load nothing but these lines —
    /// FonzAppraiser, AckisRecipeList, FonzSummon — and `embeds.xml` is the whole of their Ace
    /// dependency chain.
    ///
    /// The mixed document is the point: one `<Include>` of each kind, in one file, both landing.
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
        // ...and the XML arm is untouched: both frames still exist.
        assert_eq!(report.frames, 2);
        assert!(s
            .eval::<bool>("return FromXmlInclude ~= nil and FromMain ~= nil")
            .unwrap());
    }

    /// A raise inside an `.lua` Include is reported and the enclosing document CONTINUES — the
    /// reference logs at severity 2 and falls through (`0x6ee00d` -> `0x6ee012` is unconditional,
    /// `eax` never read), so one bad library must not cost the frames declared after it.
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

    /// `<Include>` resolved through an in-memory provider closure.
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

    /// **An unrecognised `frameStrata=` warns and skips; the frame keeps the stratum it had, and
    /// the rest of the document loads** (decision 2160).
    ///
    /// The two doors differ in the reference and this is the quiet one. `CSimpleFrame::LoadXML
    /// 0x769820` resolves through `0x6f17d0`, whose miss returns 0 without writing the out-param;
    /// the miss leg pushes `"Frame %s: Unknown frame strata: %s"` at severity 1 into the document
    /// sink (`0x7699a4`, a call that returns) and reconverges with the hit path at `0x7699ad`,
    /// never reaching `SetFrameStrata 0x76a470`. `EQL3`'s `EQL3_Log.xml` carries the literal case:
    /// `<Frame frameStrata="ARTWORK">`, a draw-LAYER name where a strata belongs.
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

    /// **The Lua door is the LOUD one and stays that way.** `SetFrameStrata 0x774360`'s miss
    /// reaches `0x774456 call 0x6f4940` (`luaL_error`), whose chain `luaG_errormsg 0x6fc780` /
    /// `luaD_throw 0x6f5d80` contains no `ret` at all — the epilogue after it is dead code. Pinned
    /// beside its XML twin so the asymmetry is a test rather than a comment.
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

    /// **`FrameXML_Debug` is a get-or-set whose SET arm turns on the loader's trace lines**
    /// (decision 2160) — and `0` takes the SET arm, because the number zero is Lua-truthy and only
    /// `nil`/`false` are not (`0x48845d`, after `lua_toboolean 0x6f34d0`). That is the half a
    /// re-implementation gets backwards, so it is the half asserted first.
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

        // The two arms that are easy to get wrong.
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
        // …and the stored value is truncated toward zero, `0x40a2b0`'s conversion.
        assert_eq!(s.eval::<i32>("return FrameXML_Debug(1.9)").unwrap(), 1);
    }

    /// A missing include is **reported, not raised**, and the rest of the load still proceeds.
    ///
    /// It was a warning until 1186 and an error from 1186 to 2155; it is now its own list. Both of
    /// the findings behind those moves are asserted here, because each undid the other:
    /// **1186's** — a document that resolved *nothing* must not report success (Bagnon missed all
    /// eleven of its references and came back with zero errors) — so the row is in the report; and
    /// **2155's** — the reference logs `Couldn't open %s` (`0x6edaa0`, `0x846ff4`) and carries on
    /// with nothing raised — so the row is *not* in `errors`, which is the list whose entries
    /// reach the player's red error dialog.
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

    /// So is a missing `<Script file=>` — it drops every handler the file would have defined, and
    /// the reference's own leg for it (`"Error loading %s"`, `0x872e50`) returns
    /// normally rather than throwing.
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

    /// **A `<Script file=>` whose chunk RAISES is still an error** — the half `missing_files` must
    /// not swallow. The two arms sit one line apart in `load_in`, and folding a miss into the
    /// quiet list is only correct because the raise keeps its own.
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

    /// `$parent` resolves in a child's *name* and in an anchor's `relativeTo`.
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
        // Child $parent-name resolved to PFInner and published.
        assert!(s.eval::<bool>("return PFInner ~= nil").unwrap());

        // relativeTo="$parent" resolved to PF, so the child sits exactly on the parent's TOPLEFT.
        s.resolve();
        let quads = s.extract();
        let rects: Vec<_> = quads
            .iter()
            .filter_map(|q| match q.target {
                ZTarget::Frame(_) => q.rect,
                _ => None,
            })
            .collect();
        // Parent: screen [0,0,600,800] TOPLEFT+(50,-50), 100x100 → Rect(450,50,550,150). The child,
        // anchored TOPLEFT→parent TOPLEFT with no offset, resolves to the same rect.
        let expected = crate::layout::Rect::new(450.0, 50.0, 550.0, 150.0);
        assert!(
            rects.iter().filter(|r| **r == expected).count() >= 2,
            "rects: {rects:?}"
        );
    }

    /// A named state texture and a named `<ButtonText>` are anchor targets by NAME, not only
    /// globals: the stock trainer row hangs its label off `$parentHighlight`'s RIGHT, and a name
    /// published only into `_G` sent the label to the button's own edge (1957).
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

    /// A handler with a syntax error yields an `errors[]` entry, the load continues, and the other
    /// frame is still built with a working handler.
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
        // The broken frame exists but its OnLoad never ran; the good frame's did.
        assert!(s.eval::<bool>("return Broken ~= nil").unwrap());
        assert!(s.eval::<bool>("return FineRan == true").unwrap());
    }

    /// Unsupported handler names are warn-once gaps, not hard errors, and don't stop the frame
    /// from building. The example is `OnAttributeChanged` — 2.0's secure-frame system, which no
    /// 1.12 resolver has a slot for, so it is the one name on that list that is out permanently.
    ///
    /// It has been three names now, and each move is the rule working: `OnKeyDown` left when 1319
    /// built the delivery walk, `OnCursorChanged` when 2141 built the caret flush's fire. A name
    /// is accepted only once something fires it, so this test's subject is whatever is still
    /// waiting.
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

    /// **An unknown frame type at the XML door is LOGGED, and its node is skipped — nothing
    /// raises** (decision 2191).
    ///
    /// `Instantiate 0x6ee280` prints `"Unknown frame type: %s"` (`0x871124`) at `0x6ee356` and makes
    /// no object for that node; only the Lua `CreateFrame` binding raises (`0x872fa8` via
    /// `luaL_error`). The node's own `<Frames>` go with it — there is no object to parent them —
    /// and the walk carries on to the next sibling.
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
        // …and the Lua door still raises, on the same lookup.
        let err = s.run(r#"CreateFrame("Bogus")"#).unwrap_err().to_string();
        assert!(err.contains("unknown frame type 'Bogus'"), "{err}");
    }

    /// **A `.toc`-listed `Bindings.xml` loads as XML and costs nothing but log lines** (decision
    /// 2191) — MonkeyDev's shape, the director's live report. The `.toc` line runner hands every
    /// non-`.lua` entry to the same file loader (`0x6edd51` → `0x6ede10`), whose walk ignores the
    /// root's own tag (`<Bindings>`, tolerated) and sees three `<Binding>` elements: three unknown
    /// frame types, logged. The file's real reading as bindings is `0x51f400`'s, which runs anyway.
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

    /// The WorldFrame's one-shot record, at the XML door: a second `<WorldFrame>` is an unknown
    /// type, so it logs and builds nothing (decisions 1984, 2191) where `CreateFrame` raises.
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

    /// `<StatusBar>` LoadXML extras (`0x782ef0`): value attributes, `<BarTexture>` + `<BarColor>`
    /// children, orientation — landing in the widget state and scaling the extracted bar quad.
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

    /// The creation-path implicit anchor, XML half (decision 1310; `0x7701c0`): a `<Texture>` with
    /// zero anchors gets SetAllPoints(parent) right after its LoadXML — two corner anchors that pin
    /// all four edges, so an authored `<Size>` is structurally unread. This is B180's engine shape:
    /// the reference stack-split plate authors a vestigial 256×32 and renders 172×96, the frame.
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

    /// The FontString half of the same law: zero anchors ⇒ ONE middle-row SetPoint chosen by the
    /// live justify word (`&7`: 1→LEFT, 4→RIGHT, else CENTER) — and the authored `<Size>` stays
    /// LIVE (single anchor + W/H sizes the opposite edges), unlike the texture's.
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
        // LEFT→LEFT at (0,0): left edge at the page's left, vertically on the page's centerline
        // (middle-row point), 80×20 from the size. Page [0,0]..[200,100] → [0,40]..[80,60].
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

    /// The XML state-texture ordering (decision 1310's loader half): an authored `<Anchors>` on a
    /// `<NormalTexture>` wins outright — the materializing setter's implicit SetAllPoints is
    /// cleared before the authored layout applies, so no implicit corner survives to weld the
    /// region to the button (the merchant row's icon-scoped highlight is the live consumer).
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

    /// **An XML handler body's chunk name is `"<GetName()>:<Handler>"`, and its lines are the
    /// body's own.** `0x7025fd` calls the script object's `GetName` through its vtable, falls back
    /// to `<unnamed>` (`0x84c7f0`) at `0x702611`, formats `"%s:%s"` (`0x872a28`) at `0x70261b`,
    /// and hands that to `0x704c70` at `0x70263c` — which `luaL_loadbuffer`s the raw body, so body
    /// line *n* is chunk line *n*.
    ///
    /// Three producers, because they used to disagree. A named element was already right; an
    /// unnamed one wrote `<Button>` where the image writes `<unnamed>`; and a `CreateFrame` off a
    /// template wrote the *whole call* —
    /// `CreateFrame("Button", "Made", inherits="ProbeTmpl"):OnClick` — into every error message
    /// and every `debugstack` frame that handler produced.
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

        // A named element. The body sits on the element's SECOND line, and the reference numbers a
        // handler chunk from the body's own first line — so `:2:`, never `:3:`. Our wrapper used
        // to end with a newline, which pushed every body line down by one.
        let named = raised(r#"NamedBtn:GetScript("OnClick")"#);
        assert!(
            named.starts_with(r#"[string "NamedBtn:OnClick"]:2: boom"#),
            "{named}"
        );
        // A `CreateFrame` instance is named by the INSTANCE — what `GetName()` answers — not by
        // the call that made it and not by the template it wore.
        let made = raised(r#"Made:GetScript("OnClick")"#);
        assert!(
            made.starts_with(r#"[string "Made:OnClick"]:1: tmpl"#),
            "{made}"
        );
        // The nameless element takes the image's own fallback literal.
        let anon = raised(r#"BenillaAnon:GetScript("OnClick")"#);
        assert!(
            anon.starts_with(r#"[string "<unnamed>:OnClick"]:1: anon"#),
            "{anon}"
        );
    }

    /// **An inline `<Script>` body is `"<xml path>:<Scripts>"`, with no `@`** — `0x6ee0ed push ebx`
    /// (the document's own path) / `0x6ee0ee push 0x871074` (`"%s:<Scripts>"`) → `0x6ee0ff` sprintf
    /// → `0x6ee10f call 0x704cd0`. `luaO_chunkid` therefore takes its *third* branch and the frame
    /// reads `[string "…"]`, not a bare path — which is how a reader (and every addon that splits a
    /// `debugstack` frame on `\AddOns\`) tells an inline block from the `.lua` file beside it.
    ///
    /// `<Script file="…">` keeps the file form, `"@%s"` (`0x8716e0`), and both are asserted here so
    /// the two arms cannot drift into each other.
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

        // The checked CheckButton draws its check mark; the idle Button draws only its normal.
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

        // A physical click fires the XML OnClick.
        s.mouse_button(30.0, 10.0, "LeftButton", true);
        s.mouse_button(30.0, 10.0, "LeftButton", false);
        assert!(s.eval::<bool>("return xml_clicked == true").unwrap());
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    /// A state texture's own `<Size>` + `<Anchors>` scope it to less than its button (the merchant
    /// row's icon-scoped highlight): hovering the 100px-wide button, the highlight quad renders at
    /// the anchored 20px square, not stretched over the button.
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

        // Hover the button's far-right end — the highlight still sits at the anchored TOPLEFT.
        // (Resolve first: hit-testing reads the resolved-rect cache.)
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

    /// `<EditBox>` LoadXML extras (`0x779fb0`): `letters` → SetMaxLetters and the config flags land
    /// in the widget state; a click focuses it and the `<OnTextChanged>` handler fires on a typed
    /// char.
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

        // letters=5 landed; numeric abort rejects a non-digit insert wholesale.
        assert_eq!(s.eval::<i64>("return XmlEdit:GetNumLetters()").unwrap(), 0);
        s.run(r#"XmlEdit:Insert("12x")"#).unwrap(); // numeric: aborts
        s.run(r#"XmlEdit:Insert("123456")"#).unwrap(); // digits, capped at 5
        assert_eq!(
            s.eval::<String>("return XmlEdit:GetText()").unwrap(),
            "12345"
        );

        // A click focuses the box (mouse-enabled by construction); a typed char marks it changed,
        // and the drain on the next tick is what actually fires OnTextChanged (decision 1831).
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

    /// **`autoFocus="false"` is an opt-OUT, and the only reason the attribute is ever written.**
    ///
    /// The ctor leaves `flags = 1` — autoFocus ON — so every one of the ten places the shipped
    /// 1.12.1 chain names the attribute names it to turn the flag *off* (MailFrame ×3,
    /// MoneyInputFrame ×3, FriendsFrame ×3, AddonList ×1). The flag loop used to call the setter
    /// only when an attribute parsed as `true`, which made all ten a no-op: harmless while our own
    /// default was off, and exactly backwards once it matches the client's (decision 1686).
    ///
    /// Both directions asserted, because a presence-blind reader passes the `true` half.
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

        // The box is hidden first so the Show() is a real transition; an autoFocus box takes the
        // keyboard on show, which is the observable the flag actually drives.
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

    // ── TexCoords + Font objects (decision 0084 engine slice) ───────────────────────────────────

    /// `<TexCoords left right top bottom>` on a `<Texture>` parses into the region's UV rect and
    /// surfaces on the extracted quad as `[left, right, top, bottom]`.
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

    /// A `<Font>` `inherits=` chain resolves height + color through two levels: the leaf inherits its
    /// face + one value from the root and overrides another mid-chain (last-wins merge).
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

        // Leaf: face inherited from Base (through Mid), color from Mid (white), height overridden (10).
        let leaf = s.font_object("Leaf").expect("Leaf registered");
        assert_eq!(leaf.font.as_deref(), Some("Fonts\\FRIZQT__.TTF"));
        assert_eq!(leaf.height, Some(10.0));
        assert_eq!(leaf.color, Some([1.0, 1.0, 1.0, 1.0]));
    }

    /// A `<FontString inherits="GameFontNormalSmall">` reports the *smaller* resolved height (and the
    /// inherited face + color) on the extracted quad — the per-FontString size the renderer bakes at.
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

        // The inherited face + color also flow through.
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

    /// An explicit `<Color>` on a FontString overrides its font object's color; an `inherits=` naming
    /// no registered font object is a warn-once gap, not a dropped region.
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
        // A's explicit <Color> wins over GameFontNormal's gold; its height still comes from the object.
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
        // B still materialized (the unknown inherits didn't drop it).
        assert!(s
            .extract()
            .into_iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "B")));
    }

    /// The chat edit box's shape: an EditBox with BOTH a `<Layers>` FontString (the "Say:" header)
    /// and the special direct-child `<FontString>` (the engine's text-font slot). The box must
    /// adopt the SPECIAL one as its text region — the engine ASSIGNS that slot at LoadXML, it
    /// never searches. (The old find-first adoption grabbed the header: typing then overwrote
    /// "Say:", and the `<TextInsets>` re-anchor clobbered the header's LEFT anchor into a
    /// full-width rect — the centered-header + invisible-typing chat bug.)
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

        // Type into the focused box.
        s.run("EB:SetFocus()").unwrap();
        assert!(s.char_input("h"));
        assert!(s.char_input("i"));

        // Resolve; answer every fontstring measure with a stand-in extent (30×12), resolve again
        // (the round-trip's second solve).
        s.resolve();
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .iter()
            .map(|r| (r.id, 30.0, 12.0, r.key))
            .collect();
        s.set_measured_text_unwrapped(&answers);
        s.resolve();

        let quads = s.extract();
        // The header still reads "Say:" — typing lands in the box's own text region, never the
        // header. Both strings render.
        let header = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Say:"))
            .expect("the header FontString still renders its own text");
        let typed = quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "hi"))
            .expect("the typed text renders in the box's text region");

        // Header rect: its own LEFT+13 anchor + the measured 30×12 extent (left-flush against the
        // box's left edge at x=100, v-centered on the 100..132 box) — NOT the insets rect.
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

    /// `<TitleRegion setAllPoints="true"/>` builds the frame's drag handle over its whole rect —
    /// the stock `TutorialFrame.xml`'s (1976): a press inside it moves the frame with the cursor,
    /// exactly as `frame:CreateTitleRegion():SetAllPoints()` from Lua does
    /// (`script::tests::movable::a_title_region_drag_swallows_the_press_and_ends_on_release`);
    /// the same frame without the element does not move.
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

    /// `<HitRectInsets>` reaches SetHitRectInsets, and the inset band stops capturing the mouse
    /// while the frame's own geometry is untouched — the ref micro-button shape (a 29x58 frame whose
    /// art fills only the lower ~40, `top="18"`).
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
        // Geometry untouched: still a 58-tall button.
        assert_eq!(s.eval::<f64>("return Micro:GetHeight()").unwrap(), 58.0);
        // A button is mouse-enabled by construction, so the hit test is live.
        assert!(
            s.hit_test(14.0, 50.0).is_none(),
            "the dead 18-unit header must not capture"
        );
        assert!(s.hit_test(14.0, 20.0).is_some(), "the art band still hits");
    }

    /// **The `<Scripts>` walker auto-enables the mouse kind when it attaches a mouse handler**
    /// (`0x769ef0` → `0x76af00(2,-1)` per handler name) — the reference's GameTimeFrame declares
    /// `<OnEnter>`/`<OnLeave>` and no
    /// `enableMouse`, yet its tooltip hovers in the real client. The kind-2 name set is exactly
    /// {OnEnter, OnLeave, OnMouseDown, OnMouseUp, OnDragStart}: `OnDragStop`/`OnReceiveDrag` are
    /// NOT in it, and the law is XML-load-time only — the Lua SetScript binding (`0x7748d0`)
    /// never auto-enables (verified negative there).
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
        // Runtime SetScript never auto-enables — the law is the XML walker's, not SetScript's.
        s.run("rt = CreateFrame('Frame', 'Rt'); rt:SetScript('OnEnter', function() end)")
            .unwrap();
        assert!(
            !s.eval::<bool>("return Rt:IsMouseEnabled()").unwrap(),
            "a runtime-created frame still needs an explicit EnableMouse"
        );
    }

    /// **`<NormalText>` is the Button's label under its second name** — the spelling the reference
    /// FrameXML never writes and a third-party template reaches for constantly.
    ///
    /// `CSimpleButton::LoadXML 0x7788c0` gives this tag strictly more than `<ButtonText>` does: it
    /// constructs the label `CSimpleFontString` (`0x778b6c call 0x770d30`), clears its
    /// `ownsFontAttrs` (`0x778b7b`, the one site image-wide that clears it) so its own `LoadXML`
    /// skips the whole font-attribute surface, and feeds the SAME node to the button's persistent
    /// Normal-state `CSimpleFont` at `+0x33c` (`0x778ba9`/`0x778baf call 0x783c30`). One owner per
    /// attribute: the label pass reads the element's name and geometry, the state-font pass reads
    /// its `inherits=`, and neither reads the other's.
    ///
    /// **Why this could sit broken indefinitely.** The reference writes `<ButtonText>` 31 times and
    /// `<NormalText>` never, so our own FrameXML could not notice the tag was unhandled — and an
    /// unhandled child element is silent: the button loads, paints, and simply has no label and no
    /// global. KLHThreatMeter's column-header template is exactly this shape, and the addon died at
    /// `PLAYER_LOGIN` indexing the `nil` global, six frames deep, naming nothing about XML.
    ///
    /// The document is that template, reduced: a virtual `Button` carrying a named `<NormalText>`,
    /// reached through `inherits=` (never directly), so this covers the template path the addon
    /// actually takes.
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

        // 1 · THE GLOBAL. `$parent` resolved against the instantiated button and published in `_G`
        // — this single read is the one KLHThreatMeter died on.
        assert!(
            s.eval::<bool>("return KtmSelfHeaderNameText ~= nil")
                .unwrap(),
            "a named <NormalText> must publish its global"
        );
        // 2 · ...and it is the BUTTON'S label, not a loose FontString that merely shares the name.
        // Naming a region and publishing a global are two different things (the `<ButtonText>` path
        // learned that the expensive way), so both directions are asserted.
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
        // 3 · The element's own geometry landed on the label — the label pass's half.
        assert_eq!(
            s.eval::<f64>("return KtmSelfHeaderNameText:GetWidth()")
                .unwrap(),
            70.0
        );
        // 4 · ...and its `inherits=` landed on the button's NORMAL-STATE font, which is the half a
        // plain `<ButtonText>` alias would have missed. Read off the painted quad rather than a
        // getter: the state font is applied at extract, so what is on screen is the only reading
        // that proves the link is live.
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

    /// **The two spellings of a Button's label are two different legs, and only `<NormalText>` is
    /// disowned of its own font attributes — so its `justifyH` never reaches the label and the
    /// implicit anchor stays CENTER, while a `<ButtonText>`'s does and still seats LEFT.**
    ///
    /// `CSimpleButton::LoadXML 0x7788c0`'s child chain routes them apart:
    /// `<ButtonText>` (tag `0x8799f0`, compared `0x7789c1`) goes to `0x7789d0 call 0x6f2780`, the
    /// ordinary `<FontString>` region builder the `<Layers>` walker uses, which never writes
    /// `+0x12c`; `<NormalText>` (tag `0x879978`, `0x778b43`) goes to the inline build
    /// `[0x778b4c, 0x778bb6)` carrying `0x778b7b mov byte [edi+0x12c],0` — the one site image-wide
    /// that clears the gate. Cleared, gate B (`0x7710d3 → je 0x771468`) makes the label's own
    /// `CSimpleFontString::LoadXML 0x770f40` skip `0x7710e1`–`0x771467` wholesale: `font=`,
    /// `<FontHeight>`, `outline=`, `monochrome=`, the file load, `spacing=`, **`justifyV=`,
    /// `justifyH=`**, `<Color>` and `<Shadow>`. That node is fed instead to the button's
    /// Normal-state `CSimpleFont` at `+0x33c` (`0x778ba9`/`0x778baf call 0x783c30`).
    ///
    /// So the creation post-step `0x771480` reads the `<NormalText>` label's **untouched ctor
    /// default** `0x212` = CENTER|MIDDLE (`0x770dd3`) and seats a single CENTER→CENTER anchor
    /// (point 4, `0x7714dc`/`0x7714e4`). The authored `justifyH="LEFT"` still reaches the *paint*
    /// and `GetJustifyH()` — `0x783c30`'s tail notify writes the resolved word into the label's own
    /// `+0x120` (`0x784111` → `0x784180` → `0x773530` → `0x770800` at `0x770876`) — but that runs
    /// downstream of the anchor, and a FontString pinned at one point is exactly as wide as its
    /// text, so a left justify inside it has nothing to move.
    ///
    /// The live case is Gatherer 1.0.0's minimap quick-menu, transcribed here: every row is a
    /// `GathererUI_PopupButtonTemplate`, all rows are `SetWidth` to one common width and stacked
    /// TOP-to-BOTTOM so they share a centreline, and the template's only alignment statement is
    /// `<NormalText inherits=… justifyH="LEFT"/>`. Applying that word to the label seated all
    /// seven rows LEFT→LEFT and the menu read as a left-aligned list against the reference's
    /// centred one.
    ///
    /// Two controls, and the second is why this test carries both spellings: an ordinary
    /// `<Layers>` `<FontString justifyH="LEFT">` still seats LEFT
    /// (`anchorless_fontstring_seats_at_its_justify_point` above), and so does a
    /// `<ButtonText justifyH="LEFT"/>` — the gate is the `<NormalText>` leg's alone.
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

        // 1 · The anchor — the whole mechanism, read straight off the label.
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

        // 2 · ...and the authored word still reaches the label's PAINT and its getter, through the
        // Normal-state font's link. Only the anchor was ever the reference's answer here.
        assert_eq!(
            s.eval::<String>("return GathererUI_PopupButton1:GetFontString():GetJustifyH()")
                .unwrap(),
            "LEFT",
            "the state font's justify still reaches the label"
        );

        // 3 · The CONTROL, and the half the byte round corrected: `<ButtonText>` is built by the
        // ordinary FontString builder (`0x6f2780`), which never clears `+0x12c`, so its own
        // justifyH lands on the label and the same post-step seats it LEFT.
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

        // 4 · The visible consequence, in resolved coordinates. The popup is [0,0]..[120,60] and
        // the rows are 64 wide, centred on x=60 → [28,92]. A 40-wide label centred in that is
        // [40,80]; seated LEFT it is [28,68] — the left edge every Gatherer row shared before this.
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

    /// **`text=` is a GLOBAL-STRING LOOKUP, not a literal** (`FrameScript_GetText 0x703bf0`). Every
    /// arm of [`Loader::resolve_text`] in one document:
    /// a `<Button text=>`, a `<ButtonText text=>` and a `<FontString text=>` all resolve through
    /// the VM's globals; a value with no matching global falls back to the LITERAL (benilla's own
    /// divergence, so its plain-English FrameXML keeps working); and a **key-shaped** miss warns,
    /// which is the tripwire that "CREATE_MACROS" across a title bar never had.
    #[test]
    fn a_text_attribute_resolves_through_the_global_strings() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        // The reference boots GlobalStrings.lua before any XML; so does the app.
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
                // The key's VALUE, on all three element shapes…
                "The Title",
                // …a non-key literal untouched (the divergence that keeps our own XML working)…
                "No results found.",
                "Delete",
                "Exit Game",
                // …and a key-shaped MISS keeps its key on screen rather than blanking.
                "NO_SUCH_KEY",
            ]
        );
        // Exactly one warning, and it names the miss — the plain-English literal must not warn.
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

    /// The key SHAPE test itself: `SCREAMING_SNAKE`, two characters or more. The floor is what
    /// keeps a close button's `text="X"` from being reported as a missing string.
    #[test]
    fn key_shape_is_screaming_snake_of_two_or_more() {
        for yes in ["DELETE", "EXIT_GAME", "CHARACTER_POINTS1_COLON", "AB", "A1"] {
            assert!(is_global_string_key(yes), "{yes} is key-shaped");
        }
        for no in ["X", "", "Send Mail", "No results found.", "Okay", "1", "12"] {
            assert!(!is_global_string_key(no), "{no} is not key-shaped");
        }
    }

    // ── <SimpleHTML> (the markup widget's XML element) ───────────────────────────────────────

    /// **`<SimpleHTML>`'s `<FontString>` child is a font DECLARATION, not a region** — the shape
    /// stock `ItemTextFrame.xml` uses, and the whole reason its `<H1>` renders at the `<P>` size.
    ///
    /// Four claims in one document: the element materializes as a real `SimpleHTML` frame with its
    /// `<Size>`/`<Anchors>`/`<Scripts>` applied like any frame; the direct-child `<FontString
    /// inherits=>` lands on `elementFont[0]` rather than creating a FontString region; a
    /// `<FontStringHeader1>` lands on `elementFont[1]`; and `hyperlinkFormat=` reaches `+0x360`.
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
        // The two declarations landed on their own elements…
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
        // …and NEITHER became a region. A `<FontString>` handled by the generic special-fontstring
        // pass would leave an unanchored, textless string on the frame here.
        let regions = {
            let lua = s.lua();
            let model = lua
                .app_data_ref::<crate::script::Model>()
                .expect("model app_data");
            let fh = model.arena.lookup("PageText").expect("frame");
            model.arena.frame(fh).expect("live").regions.len()
        };
        assert_eq!(regions, 0, "the font declarations created no regions");

        // Now drive it: three blocks, the H1 in its OWN declared face, at the frame's width.
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

    /// A state-texture element with **no `file=`, no `<Color>` and no `name=`** still builds its
    /// region: `<NormalTexture/>` is the reference's own form on `SpellBookSkillLineTabTemplate`
    /// (ref SpellBookFrame.xml l.36), and the real client creates from the ELEMENT, not from any
    /// attribute on it — `Button::LoadXML 0x7788c0` routes all four `<...Texture>` children through
    /// the same texture adder the `<Layers>` walker uses (`0x6f26f0`), each followed only by the slot
    /// store, which does no geometry.
    ///
    /// The report is pfUI's spellbook skin, which takes `SpellBookSkillLineTab<i>:GetNormalTexture()`
    /// and immediately `:SetTexCoord()`s it; the corpus carries 38 bare `<DisabledTexture />` besides.
    /// Before this, the loader materialized only on `file=`/`<Color>`/`name=`, so the getter answered
    /// nil where the real client answers a live blank texture.
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
        // Created, not painted: the region is live enough to take SetTexCoord (pfUI's next call) and
        // still carries no art of its own.
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

    /// `alphaMode="ADD"` on a `<Layers>` texture reaches the extracted quad's blend flag — the
    /// state-texture path always applied it, the layer path silently dropped it (the quest log's
    /// selection glow rendered as an opaque gray bar — the 0109 look fix).
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

    /// A `<Texture inherits="…">` layer region splices its virtual template — file, `<Size>`,
    /// `<Anchors>` — with the instance's own nodes winning (the talent branch/arrow art pool's
    /// shape; before this, the template was silently ignored and every pooled texture sat at
    /// 0×0 with no file: shown, positioned by the runtime SetPoint, and invisible).
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
        // The template's size/file landed; the instance's own <Size> wins over the template's.
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

    /// A `<FontString inherits="GameFontNormal">` names a FONT OBJECT, not an element template —
    /// the region-template gate must pass it through untouched (no "unknown template" warning).
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
        // …and the font it inherited is READABLE BACK, which is the half this test did not ask.
        // `EQL3_Options.lua:1086` is the corpus line that needs it: it reads the tracker line's
        // font with `t1, _, t2 = EQL3_QuestWatchLine1:GetFont()` and feeds `t1` straight into
        // `temp:SetFont(t1, height, t2)` — so a nil path there is not a cosmetic gap, it is our own
        // faithful `Usage: <FontString>:SetFont(...)` raise firing on our own nil.
        assert_eq!(
            s.eval::<(String, f32)>("local f, h = Label:GetFont() return f, h")
                .unwrap(),
            ("Fonts\\FRIZQT__.TTF".to_string(), 12.0),
            "a FontString that inherits a Font object reports that object's font"
        );
    }

    /// **A font-object name resolves case-INSENSITIVELY**, because the client's font registry
    /// compares its keys with `SStrCmpI` (`0x783870`/`0x7838c7`).
    ///
    /// `Recap/RecapOptions.xml:32` inherits `GameFontHighLightSmall`; the shipped font is
    /// `GameFontHighlightSmall`, one letter's case apart. On the real client that resolves, and it
    /// used to be a warn-once gap here — a NARROWING fix, like the unit-token fold (1247).
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

    /// **EQL3's actual shape: FontString -> virtual FontString TEMPLATE -> Font object.** One hop
    /// works (above); this is the two-hop chain every "define a line template once, stamp it N
    /// times" addon writes, and `EQL3_Tracker.xml:6/32` is the corpus instance —
    /// `EQL3_QuestWatch_FontTemplate` inherits `GameFontHighlight`, and each
    /// `EQL3_QuestWatchLine<i>` inherits the template.
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

/// Chunk naming — which FILE and which LINE a `<Script>` raise reports (decision 1217).
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

    /// **A raise inside a `<Script>` block names the file it came from, and the file's own line.**
    ///
    /// Both halves are load-bearing and both were wrong. `Loader::run` loaded every chunk with no
    /// name, and mlua's `load` is `#[track_caller]` — so the chunk was named after the *Rust* line
    /// that ran it, and a corpus read-back of 70 failures had 26 rows saying
    /// `crates/benilla-ui/src/loader/mod.rs:327` where the addon's file belonged (decision 1217).
    ///
    /// Naming it is only half. A Lua chunk starts at line 1, so an inline block 40 lines down a
    /// file would report `File.xml:3` for its third line — a lie you can go and check, which is
    /// strictly worse than the Rust path that was at least obviously not yours. The chunk is padded
    /// to the block's own offset, so the number is the number you scroll to.
    #[test]
    fn a_script_raise_names_its_own_file_and_line() {
        let s = UiScript::new().unwrap();
        // `error()` with no level prefixes "chunkname:line:" — exactly what a real raise carries.
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
        // The file, in the shape an addon's own `debugstack()` matches (backslashes, no `@`).
        // The `:<Scripts>` suffix and the `[string "…"]` wrapper are the reference's own
        // (`"%s:<Scripts>"` `0x871074`, `luaO_chunkid`'s third branch), not decoration of ours.
        assert!(
            err.contains("Bagnon\\src\\main.xml:"),
            "the raise must name the document, got: {err}"
        );
        // Line 5 of the literal above is `error("boom")` — the body starts on line 4 and the
        // padding carries it there. Without the pad this reads `:2:`.
        assert!(
            err.contains("main.xml:<Scripts>\"]:5:"),
            "the line must be the FILE's line, not the block's, got: {err}"
        );
    }

    /// The same, one level down: an `<Include>`d document's inline script names **its own** file,
    /// not the includer's. The path is saved and restored around the recursion exactly as the
    /// directory used to be, so depth-1 and depth-5 each report themselves.
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

    /// **The CDATA shape, which is the only shape our own FrameXML uses.** `<![CDATA[` sits
    /// between the element's start and the body's first byte, so taking the line from the
    /// *element* would be short by however much of the opening tag precedes it — and a
    /// single-line-offset lie is the hardest kind to notice. The line comes from the text child's
    /// own range instead, which is what this pins.
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

    /// A `<Script file=>` chunk is named after the file it loaded, not the document that named it.
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

    /// **A captured script, called back the reference's way, still knows its frame.**
    ///
    /// This is *the* 1.12 hook idiom and the whole reason `GetScript` exists: an addon takes the
    /// client's handler, installs its own, and calls the original with **no arguments**, because
    /// the reference's contract passes the frame as the `this` global and nothing else. Our
    /// wrapper is `function(self, ...)` and our own FrameXML is written against `self`, so before
    /// the fallback in [`super::Loader::compile_handler`] that re-entry handed every body a nil
    /// frame. The director met it as `bad argument #2: error converting Lua nil to table` out of
    /// `GameTooltip:SetOwner`, from a handler that works perfectly when the engine calls it.
    ///
    /// Both spellings are asserted, because the fallback must not cost the engine's own call.
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

        // The engine's own call: `self` is passed and `this` is set — the modern spelling.
        s.run("SEEN = nil Hooked:Show()").unwrap();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "Hooked");

        // The addon's: capture the script, then call it back bare with only `this` standing in for
        // the frame — exactly `Bagnon.lua:87`'s `bMainBag_OnEnter()`.
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

/// [`super::join_ref`] — the FrameXML path rule (decision 1186).
///
/// A reference is relative to the directory of the file containing it, `\` and `/` are the same
/// separator, and `..` walks up. The escape case is the load-bearing one: a `..` above the root is
/// KEPT as a leading `..` rather than clamped away, because the provider has to be able to SEE the
/// escape to refuse it. Clamping would silently hand it a path that looks contained.
#[test]
fn join_ref_resolves_relative_framexml_paths() {
    use super::join_ref;
    // Relative to the including file's own directory.
    assert_eq!(
        join_ref("Bagnon/src", "templates.xml"),
        "Bagnon/src/templates.xml"
    );
    // Backslashes are separators, and `..` walks up — this is how a library addon is reached.
    assert_eq!(
        join_ref("Bagnon/src", "..\\..\\BagBrother\\core\\core.xml"),
        "BagBrother/core/core.xml"
    );
    // At the root, a bare name stays bare (the builtin's flat tree).
    assert_eq!(join_ref("", "Fonts.xml"), "Fonts.xml");
    // `.` is a no-op; redundant separators collapse.
    assert_eq!(join_ref("a/b", "./c//d.xml"), "a/b/c/d.xml");
    // An escape above the root SURVIVES, so the provider can refuse it.
    assert_eq!(join_ref("a", "../../secret"), "../secret");
    assert_eq!(join_ref("", "../secret"), "../secret");
    // A leading `/` re-roots rather than meaning the filesystem root.
    assert_eq!(join_ref("a/b", "/c.xml"), "c.xml");
}
