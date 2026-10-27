use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::script::binding_abi::{s_str_to_float, s_str_to_int};
use crate::script::LabelFont;

use super::regions::FontAttrs;
use super::{children_named, color_of, Loader};

impl Loader<'_> {
    /// The type's own part of one level (each type's `LoadXML`, layout slot 8, which runs the base
    /// load `0x769820` on the same node first), chosen by `kind`, the instance's type: the virtual
    /// call is on the object, so a Frame wearing a `<Button>` template reads no Button part. The
    /// game-side types (GameTooltip, Minimap, MovieFrame, TaxiRouteFrame, WorldFrame) and a plain
    /// Frame have none.
    pub(super) fn typed_level(
        &mut self,
        level: &Element,
        kind: &str,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        let is = |t: &str| kind.eq_ignore_ascii_case(t);
        // A `<LootButton>` loads exactly as a Button: its `LoadXML` slot holds `CSimpleButton`'s
        // pointer (`0x804594[2]` = `0x81c7c8[2]`).
        if is("Button") || is("LootButton") || is("CheckButton") {
            self.button_level(level, wrapper, self_name, dbg);
            if is("CheckButton") {
                self.checkbutton_level(level, wrapper, self_name, dbg);
            }
        } else if is("EditBox") {
            self.editbox_level(level, wrapper, dbg);
        } else if is("MessageFrame") || is("ScrollingMessageFrame") {
            self.messageframe_level(level, is("ScrollingMessageFrame"), wrapper, dbg);
        } else if is("SimpleHTML") {
            self.simplehtml_level(level, wrapper, dbg);
        } else if super::model_kind_tag(kind) {
            self.model_level(level, wrapper, dbg);
        } else if is("ScrollFrame") {
            self.scrollframe_level(level, wrapper, self_name, dbg);
        } else if is("Slider") {
            self.slider_level(level, wrapper, self_name, dbg);
        } else if is("StatusBar") {
            self.statusbar_level(level, wrapper, self_name, dbg);
        } else if is("ColorSelect") {
            self.colorselect_level(level, wrapper, self_name, dbg);
        }
    }

    /// `<ScrollFrame>`'s part (`LoadXML 0x786bc0`): the level's first `<ScrollChild>` alone
    /// (`0x6f2c50`), whose first child is built through `Instantiate 0x6ee280` (`0x786bf4`), a
    /// whole frame lifecycle with the child's own `<Frames>` and OnLoad, and only then stored with
    /// `SetScrollChild` (`0x786ce0`), which marks the frame's scroll range. So the child's OnLoad
    /// sees no scroll child yet, and an instance's element, read after its template's level,
    /// wins. A `<ScrollChild>` with no child logs `0x87a68c` at severity 1 (`0x786c0c`-`0x786c30`).
    fn scrollframe_level(&mut self, level: &Element, wrapper: &Table, self_name: &str, dbg: &str) {
        let Some(sc) = children_named(level, "ScrollChild").next() else {
            return;
        };
        let Some(child) = sc.children.first() else {
            let name = self.object_name(wrapper);
            self.status_warning(format!(
                "Frame {name}: Scroll frame created without scroll child"
            ));
            return;
        };
        let Some(child_wrapper) = self.materialize(child, Some(wrapper), self_name) else {
            return; // materialize already reported why
        };
        if let Err(e) = wrapper.call_method::<()>("SetScrollChild", child_wrapper) {
            self.report.fault(format!("{dbg}: <ScrollChild>: {e}"));
        }
    }

    /// `<StatusBar>`'s part of one level (`0x782ef0`, after the base load at `0x782f06`): the
    /// level's `drawLayer`, ARTWORK without one (`0x782f12`); one child loop in document order over
    /// `<BarTexture>`, built as any texture element is and installed as the bar on that layer
    /// (`0x782f5b`, `0x782f63`), and `<BarColor>`, the bar's tint (`0x782f8d`); then the range,
    /// value and orientation. Each value with `SStrToFloat`, present and non-empty: the range
    /// needs both bounds on the node, and `defaultValue` is read only after it
    /// (`0x782fb0`-`0x783021`). The setter `0x783270` clamps a min above the max down to it. The
    /// level's `<Scripts>` were read in its base load, so the value's fire runs this level's
    /// handler.
    fn statusbar_level(&mut self, level: &Element, wrapper: &Table, self_name: &str, dbg: &str) {
        let layer = level.attr("drawLayer").filter(|v| !v.is_empty());
        for child in &level.children {
            if child.tag.eq_ignore_ascii_case("BarTexture") {
                let draw = layer.unwrap_or("ARTWORK");
                if let Some(tex) = self.build_texture_child(child, wrapper, self_name, draw, dbg) {
                    self.install_texture(wrapper, "BarTexture", &tex, layer, dbg);
                }
            } else if child.tag.eq_ignore_ascii_case("BarColor") {
                self.call(
                    wrapper,
                    "SetStatusBarColor",
                    color_of(child).fractions(),
                    dbg,
                );
            }
        }
        let value = |attr: &str| {
            level
                .attr(attr)
                .filter(|v| !v.is_empty())
                .map(super::xml_float)
        };
        if let (Some(min), Some(max)) = (value("minValue"), value("maxValue")) {
            self.call(wrapper, "SetMinMaxValues", (min, max), dbg);
            if let Some(v) = value("defaultValue") {
                self.call(wrapper, "SetValue", v, dbg);
            }
        }
        // `orientation` through the token parse `0x6f1a20`: empty is skipped, a miss logged
        // (`0x87a3c4`) and never raised (`0x783064`), where the Lua setter raises.
        self.level_orientation(level, wrapper, dbg);
    }

    /// A Slider's or StatusBar's `orientation=` on one level: empty is skipped, a miss logged
    /// (`0x87a3c4`) and never raised, where the Lua setter raises.
    fn level_orientation(&mut self, level: &Element, wrapper: &Table, dbg: &str) {
        if let Some(token) = level.attr("orientation").filter(|v| !v.is_empty()) {
            match crate::script::region::orientation(token) {
                Some(vertical) => {
                    let o = if vertical { "VERTICAL" } else { "HORIZONTAL" };
                    self.call(wrapper, "SetOrientation", o, dbg);
                }
                None => {
                    let name = self.object_name(wrapper);
                    self.status_warning(format!(
                        "Frame {name}: Unknown orientation {token} in element {}",
                        level.tag
                    ));
                }
            }
        }
    }

    /// `<Slider>`'s part of one level (`0x789580`, after the base load at `0x789596`), VERTICAL
    /// from the ctor: the level's `drawLayer`, the thumb's layer, OVERLAY without one
    /// (`0x7895a2`); each `<ThumbTexture>` in document order (`0x7895c1`-`0x789603`), built as any
    /// texture element is (a named thumb is published, which `ScrollFrame_OnScrollRangeChanged`
    /// reads by name) and installed as the thumb on that layer, the install clearing its anchors
    /// (`0x789893`); then the range, step, value and orientation. Each value through `SStrToFloat`
    /// (`0x64aaa0`), present and non-empty, so `"100%"` is 100: the range needs both bounds on the
    /// node and is not swapped, and `valueStep` and `defaultValue` are read only after it
    /// (`0x789605`-`0x7896a2`); the orientation miss is `0x78970d`.
    fn slider_level(&mut self, level: &Element, wrapper: &Table, self_name: &str, dbg: &str) {
        let layer = level.attr("drawLayer").filter(|v| !v.is_empty());
        for raw in children_named(level, "ThumbTexture") {
            let draw = layer.unwrap_or("OVERLAY");
            if let Some(tex) = self.build_texture_child(raw, wrapper, self_name, draw, dbg) {
                self.install_texture(wrapper, "ThumbTexture", &tex, layer, dbg);
            }
        }
        let value = |attr: &str| {
            level
                .attr(attr)
                .filter(|v| !v.is_empty())
                .map(|v| s_str_to_float(v) as f32)
        };
        if let (Some(min), Some(max)) = (value("minValue"), value("maxValue")) {
            self.call(wrapper, "SetMinMaxValues", (min, max), dbg);
            if let Some(step) = value("valueStep") {
                self.call(wrapper, "SetValueStep", step, dbg);
            }
            if let Some(v) = value("defaultValue") {
                self.call(wrapper, "SetValue", v, dbg);
            }
        }
        self.level_orientation(level, wrapper, dbg);
    }

    /// `<Button>`'s part of one level (`CSimpleButton::LoadXML 0x7788c0`, after the base load at
    /// `0x7788d6`): one child loop in document order (`0x7788e6`-`0x778bfe`), so a label anchored
    /// to a state texture finds it when the texture comes first; then the level's `text=`.
    /// `$parent` in a child's anchors is this button.
    fn button_level(&mut self, level: &Element, wrapper: &Table, self_name: &str, dbg: &str) {
        for child in &level.children {
            let tag = child.tag.as_str();
            let is = |t: &str| tag.eq_ignore_ascii_case(t);
            if let Some(slot) = [
                "NormalTexture",
                "PushedTexture",
                "DisabledTexture",
                "HighlightTexture",
            ]
            .into_iter()
            .find(|t| is(t))
            {
                self.button_texture(child, slot, wrapper, self_name, dbg);
            } else if is("ButtonText") {
                // The ordinary FontString builder (`0x7789d0` → `0x6f2780`).
                self.button_label(child, FontAttrs::Own, wrapper, self_name, dbg);
            } else if is("NormalText") {
                // An inline copy that disowns its font attributes (`0x778b7b`), then the node
                // loads into the Normal font (`0x778baf`).
                self.button_label(child, FontAttrs::Disowned, wrapper, self_name, dbg);
                self.button_font(child, LabelFont::Normal, wrapper, dbg);
            } else if is("NormalFont") {
                self.button_font(child, LabelFont::Normal, wrapper, dbg);
            } else if is("HighlightFont") || is("HighlightText") {
                // `<HighlightText>` and `<DisabledText>` build no region, only fonts (`0x778bf4`).
                self.button_font(child, LabelFont::Highlight, wrapper, dbg);
            } else if is("DisabledFont") || is("DisabledText") {
                self.button_font(child, LabelFont::Disabled, wrapper, dbg);
            } else if let Some((_, method)) = [
                ("NormalColor", "SetTextColor"),
                ("HighlightColor", "SetHighlightTextColor"),
                ("DisabledColor", "SetDisabledTextColor"),
            ]
            .into_iter()
            .find(|(t, _)| is(t))
            {
                // Each colour element (`0x6f23d0`) is its font's text colour. Deviation:
                // `<HighlightColor>` and `<DisabledColor>` colour their own fonts; 1.12.1 sends
                // both to the normal colour setter (`0x778aa4`, `0x778ada` call `0x778e20`), so
                // neither could reach its state.
                self.call(wrapper, method, color_of(child).fractions(), dbg);
            } else if is("PushedTextOffset") {
                // `0x778aec`: the dimension read (`0x6f1eb0`), stored as `SetPushedTextOffset`
                // does (`0x778b24`) only when it succeeds (`0x778b0c`).
                let d = self.dimension(child);
                if d.ok {
                    let [x, y] = d.script(self.model().layout_units());
                    self.call(wrapper, "SetPushedTextOffset", (x, y), dbg);
                    crate::script::xml_layout::pushed_text_offset(self.lua, wrapper, d.v);
                }
            }
        }
        // The level's `text=`, a global-string key (`0x703bf0`, `text="DELETE"` reads "Delete"),
        // an empty one skipped (`0x778c1a`), set on the label the loop left (`0x778dc0`).
        if let Some(text) = level.attr("text").filter(|t| !t.is_empty()) {
            let text = self.resolve_text(text, dbg);
            self.call(wrapper, "SetText", text, dbg);
        }
    }

    /// `<CheckButton>`'s part of one level (`0x785170`): the Button part (a direct call to
    /// `0x7788c0`, so its base load re-enters this type's slot for a template), then `checked`
    /// through `0x6f1b30`, a missing or empty one skipped (`0x78518e`-`0x78519d`), then one
    /// document-order loop over the two checked textures (`0x7851b5`-`0x785214`).
    fn checkbutton_level(&mut self, level: &Element, wrapper: &Table, self_name: &str, dbg: &str) {
        if let Some(checked) = level.attr_bool_opt("checked") {
            self.call(wrapper, "SetChecked", checked, dbg);
        }
        for child in &level.children {
            if let Some(slot) = ["CheckedTexture", "DisabledCheckedTexture"]
                .into_iter()
                .find(|t| child.tag.eq_ignore_ascii_case(t))
            {
                self.button_texture(child, slot, wrapper, self_name, dbg);
            }
        }
    }

    /// One state-texture element: a new region from the `<Layers>` texture adder (`0x6f26f0`,
    /// `0x778903`), named, laid out, painted and given its implicit all-points as any layer
    /// texture is, then handed to the slot store, which destroys the region it displaces. An
    /// instance's element over its template's therefore starts fresh: nothing of the template's
    /// region survives it. The Lua path leg differs: it repaints the slot's region and keeps its
    /// state (`0x778f10`).
    fn button_texture(
        &mut self,
        raw: &Element,
        slot: &str,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        let Some(region) = self.build_texture_child(raw, wrapper, self_name, "ARTWORK", dbg) else {
            return; // build_texture_child already reported why
        };
        self.install_texture(wrapper, slot, &region, None, dbg);
        // Freeing the displaced region took its entry out of the name lookup, which a
        // replacement of the same name takes back. The replacement's `GetName()` is its own
        // (`0x76c650`); `_G` keeps its first holder (`0x701c8a`).
        if let Some(rname) = raw.name().map(|n| framexml::resolve_name(n, self_name)) {
            crate::script::region::publish_region_name(self.lua(), &rname, &region);
        }
    }

    /// One label element, `<ButtonText>` or `<NormalText>`: the button's label, loaded level by
    /// level as any FontString is (`0x6f2780` → `0x770f40`, `0x778b8f`), `<NormalText>` skipping
    /// its font attributes (`0x778b7b`).
    fn button_label(
        &mut self,
        bt: &Element,
        font_attrs: FontAttrs,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        // `SetText` creates the label even with no text, so its layout lands on a real region;
        // `text=` is each level's, a global-string key (`0x703bf0`), an empty one skipped
        // (`0x77101b`).
        let label_levels = self.fontstring_levels(bt);
        let mut label = String::new();
        for level in &label_levels {
            if let Some(raw) = level.attr("text").filter(|t| !t.is_empty()) {
                label = self.resolve_text(raw, dbg);
            }
        }
        // Each element is a new label, named or not (`0x6f2780`, `0x778b6c`), which the install
        // puts in place of the one before, destroying it (`0x778d38`-`0x778d3c`): a template
        // level's label never survives an instance's element. A named one is created as a named
        // region, not aliased after `SetText`, so its `GetName()` answers.
        let bt_name = bt.name().map(|raw| framexml::resolve_name(raw, self_name));
        match wrapper.call_method::<Table>(
            "CreateFontString",
            (bt_name.clone(), Option::<String>::None),
        ) {
            Ok(fs) => self.call(wrapper, "SetFontString", fs, dbg),
            Err(e) => self
                .report
                .warnings
                .push(format!("{dbg}: ButtonText CreateFontString: {e}")),
        }
        self.call(wrapper, "SetText", label, dbg);
        let Ok(region) = wrapper.call_method::<Table>("GetFontString", ()) else {
            return;
        };
        // Clear, lay out, then the implicit step, as both reference legs do (`0x6f27f5`,
        // `0x778b96`); the button's own adopter anchor never fires on them (`0x778d5f`). On
        // `<NormalText>` the justify is the Normal font's, so the implicit anchor reads the ctor's
        // CENTER (`0x212`); the justify still reaches the paint (`0x784111`).
        self.call_region(&region, "ClearAllPoints", (), dbg);
        for level in &label_levels {
            self.load_fontstring_level(level, &region, self_name, font_attrs, false, dbg);
        }
        if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region) {
            self.report.fault(format!("{dbg}: implicit anchor: {e}"));
        }
        // Then the install's refresh (`0x778daa`): the label follows the state's font, not the
        // font its own `inherits=` or `font=` linked.
        if let Err(e) = crate::script::refresh_label_lua(self.lua, wrapper) {
            self.report.fault(format!("{dbg}: label refresh: {e}"));
        }
        // Published both ways, like a named state texture.
        if let Some(rname) = bt_name {
            crate::script::region::publish_region_name(self.lua(), &rname, &region);
            if let Err(e) = self.lua().globals().set(rname.clone(), region) {
                self.report
                    .warnings
                    .push(format!("{dbg}: ButtonText global '{rname}': {e}"));
            }
        }
    }

    /// One of the three embedded fonts, loaded from the element as a `<Font>` is (`0x778baf`,
    /// `0x778bf4` → `0x783c30`).
    fn button_font(&mut self, el: &Element, which: LabelFont, wrapper: &Table, dbg: &str) {
        match crate::script::state_font_lua(self.lua, wrapper, which) {
            Ok(id) => self.load_font_node(el, id, "<unnamed>"),
            Err(e) => self.report.fault(format!("{dbg}: <{}>: {e}", el.tag)),
        }
    }

    /// `font=` on an EditBox, a MessageFrame or a ScrollingMessageFrame (`0x779fcb`, `0x78592b`,
    /// `0x787b3b`): a non-empty name found in the font table (`0x783870`) links the frame's own
    /// font to it (`0x770c60`); any other is logged ([`Self::report_font_miss`]).
    pub(super) fn apply_frame_font(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        let Some(font) = el.attr("font").filter(|f| !f.is_empty()) else {
            return;
        };
        let to = self.model().fonts.id(font);
        if let Some(to) = to {
            match crate::script::own_font_lua(self.lua, wrapper) {
                Ok(Some(own)) => self.link_font_node(own, to, "<unnamed>"),
                Ok(None) => {}
                Err(e) => self.report.fault(format!("{dbg}: font=: {e}")),
            }
        } else {
            self.report_font_miss(el, wrapper, font);
        }
    }

    /// A frame's `font=` that names no font: `%s %s: Couldn't find font object named %s`
    /// (`0x879bc8`) with the frame's type (vtable slot 7), its name or `<unnamed>`, and the value.
    fn report_font_miss(&mut self, el: &Element, wrapper: &Table, font: &str) {
        let kind: String = wrapper
            .call_method("GetObjectType", ())
            .unwrap_or_else(|_| el.tag.clone());
        let name = self.object_name(wrapper);
        self.status_warning(format!(
            "{kind} {name}: Couldn't find font object named {font}"
        ));
    }

    /// One `<FontString>` arm of an EditBox's, a MessageFrame's or a ScrollingMessageFrame's child
    /// loop, loaded into the frame's own font by the Font loader (`0x783c30` at `0x77a220`,
    /// `0x785af5`, `0x787cfe`), never a region: over a font that already follows another it first
    /// logs `%s: FontString element overriding font %s` (`0x879b3c`) with the frame's name and the
    /// followed font's. An EditBox's then caps its bytes (`0x77a225`), and a message frame's sets
    /// the length of its lines' text buffers.
    fn text_font_child(&mut self, child: &Element, editbox: bool, wrapper: &Table, dbg: &str) {
        let own = match crate::script::own_font_lua(self.lua, wrapper) {
            Ok(Some(own)) => own,
            Ok(None) => return,
            Err(e) => {
                self.report.fault(format!("{dbg}: <FontString>: {e}"));
                return;
            }
        };
        let followed = {
            let model = self.model();
            model.fonts.get(own).parent.map(|p| {
                model
                    .fonts
                    .get(p)
                    .name
                    .clone()
                    .unwrap_or_else(|| "<unnamed>".to_string())
            })
        };
        if let Some(font) = followed {
            let name = self.object_name(wrapper);
            self.status_warning(format!("{name}: FontString element overriding font {font}"));
        }
        self.load_font_node(child, own, "<unnamed>");
        if editbox {
            let text = crate::script::editbox_text_region_wrapper(self.lua(), wrapper);
            if let Some(text) = text {
                if let Err(e) = crate::script::adopt_text_region(self.lua(), wrapper, &text) {
                    self.report.fault(format!("{dbg}: adopt text region: {e}"));
                }
            }
            if let Some(bytes) = child.attr("bytes") {
                if let Err(e) = crate::script::apply_xml_bytes(self.lua(), wrapper, bytes) {
                    self.report.fault(format!("{dbg}: FontString bytes: {e}"));
                }
            }
        } else if let Some(bytes) = child.attr("bytes") {
            // A message frame's lines (`0x785afa`, `0x787d03`).
            if let Err(e) = crate::script::set_message_xml_bytes(self.lua(), wrapper, bytes) {
                self.report.fault(format!("{dbg}: FontString bytes: {e}"));
            }
        }
    }

    /// `<EditBox>`'s part of one level (`0x779fb0`, after the base load at `0x779fc6`): `font=`
    /// (`0x779fcb`), then `letters`, `blinkSpeed`, `autoFocus`, `multiLine`, `numeric`, `password`,
    /// `ignoreArrows` and `historyLines`, each skipped when absent or empty, then one child loop in
    /// document order over `<FontString>`, `<HighlightColor>` and `<TextInsets>`
    /// (`0x77a1a0`-`0x77a300`). An absent flag keeps the ctor's, where `autoFocus` is on and the
    /// rest off (`0x779a29`, `0x77a0b3`), so `autoFocus="false"` clears and an empty one leaves a
    /// template's. `blinkSpeed` is the caret half-period (`E+0x370`).
    fn editbox_level(&mut self, level: &Element, wrapper: &Table, dbg: &str) {
        self.apply_frame_font(level, wrapper, dbg);
        // `letters` through `SStrToInt` (`0x77a05c`), `blinkSpeed` through `SStrToFloat` and
        // stored as 0 unless above it (`0x77a07f`-`0x77a09f`).
        if let Some(v) = level.attr("letters").filter(|v| !v.is_empty()) {
            self.call(wrapper, "SetMaxLetters", s_str_to_int(v), dbg);
        }
        if let Some(v) = level.attr("blinkSpeed").filter(|v| !v.is_empty()) {
            let s = super::xml_float(v);
            self.call(wrapper, "SetBlinkSpeed", if s > 0.0 { s } else { 0.0 }, dbg);
        }
        for (attr, method) in [
            ("autoFocus", "SetAutoFocus"),
            ("multiLine", "SetMultiLine"),
            ("numeric", "SetNumeric"),
            ("password", "SetPassword"),
            // One flag behind both spellings (`0x77a6b0`, `0x7996e0`, bit 0x10).
            ("ignoreArrows", "SetAltArrowKeyMode"),
        ] {
            if let Some(on) = level.attr_bool_opt(attr) {
                self.call(wrapper, method, on, dbg);
            }
        }
        // `historyLines` through `SStrToInt` and 0 unless above it (`0x77a175`-`0x77a18d`), by the
        // loader's own setter (`0x77cd90`), not the Lua binding's, which refuses 0.
        if let Some(v) = level.attr("historyLines").filter(|v| !v.is_empty()) {
            if let Err(e) = crate::script::set_history_lines(self.lua, wrapper, s_str_to_int(v)) {
                self.report.fault(format!("{dbg}: SetHistoryLines: {e}"));
            }
        }
        for child in &level.children {
            let is = |t: &str| child.tag.eq_ignore_ascii_case(t);
            if is("FontString") {
                self.text_font_child(child, true, wrapper, dbg);
            } else if is("HighlightColor") {
                // The `<Color>` reader (`0x6f23d0`, `0x77a292`), then the selection's setter
                // (`0x77a2a2`).
                if let Err(e) =
                    crate::script::set_selection_color(self.lua, wrapper, color_of(child))
                {
                    self.report.fault(format!("{dbg}: <HighlightColor>: {e}"));
                }
            } else if is("TextInsets") {
                // The inset read (`0x77a2d5`), set when it succeeds (`0x77a2da`).
                if let Some(v) = self.inset(child) {
                    let [l, r, t, b] = self.script_of(v);
                    self.call(wrapper, "SetTextInsets", (l, r, t, b), dbg);
                    crate::script::xml_layout::text_insets(self.lua, wrapper, v);
                }
            }
        }
    }

    /// A Minimap's post-load part (`0x4ee2b0`, after `0x76a2f0`): the two model files for the
    /// ctor's nine `Model` children, read on the instance's own node alone (`0x4ee2c5`,
    /// `0x4ee2f6`), the stock model taking an absent attribute's place only (`0x4ee2d3`,
    /// `0x4ee304`), so an empty one goes to the loader as it is: the arrow model's makes the error
    /// cube and answers 1 (`0x707350` → `0x70737f`), the player model's clears it and answers 0
    /// (`0x76ccee`, `0x4ee286`). A 0 is logged at severity 1 (`0x84c73c`, `0x84c6fc`).
    pub(super) fn apply_minimap(&mut self, own: &Element, wrapper: &Table, dbg: &str) {
        let arrow = own
            .attr("minimapArrowModel")
            .unwrap_or(crate::widget::MINIMAP_DEFAULT_ARROW_MODEL);
        let player = own
            .attr("minimapPlayerModel")
            .unwrap_or(crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL);
        match crate::script::apply_minimap_model_attrs(self.lua, wrapper, arrow, player) {
            Ok(failed) => {
                for line in failed {
                    self.status_warning(line.to_string());
                }
            }
            Err(e) => self.warn_once(
                "minimap:models",
                format!("{dbg}: <Minimap> model attributes: {e}"),
            ),
        }
    }

    /// `<ScrollingMessageFrame>`'s and `<MessageFrame>`'s part of one level, two loaders
    /// (`0x787b20`, `0x785910`), each after the base load: `font=`, `fade`, `displayDuration` and
    /// `fadeDuration` (only when > 0), then the scrolling one's `maxLines` (> 0) or the plain one's
    /// `insertMode`, then one child loop in document order over `<FontString>` and `<TextInsets>`.
    fn messageframe_level(&mut self, level: &Element, scrolling: bool, wrapper: &Table, dbg: &str) {
        self.apply_frame_font(level, wrapper, dbg);
        // `0x6f1b30`, a missing or empty one skipped (`0x7859b2`-`0x7859bd`).
        if let Some(on) = level.attr_bool_opt("fade") {
            self.call(wrapper, "SetFading", on, dbg);
        }
        // Each a non-empty value through `SStrToFloat`, above 0 (`0x7859de`, `0x785a15`).
        for (attr, verb) in [
            ("displayDuration", "SetTimeVisible"),
            ("fadeDuration", "SetFadeDuration"),
        ] {
            if let Some(v) = level.attr(attr).filter(|v| !v.is_empty()) {
                let s = super::xml_float(v);
                if s > 0.0 {
                    self.call(wrapper, verb, s, dbg);
                }
            }
        }
        if scrolling {
            // A non-empty `maxLines` through `SStrToInt`, above 0 (`0x787c5e`-`0x787c71`).
            if let Some(v) = level.attr("maxLines").filter(|v| !v.is_empty()) {
                let n = s_str_to_int(v);
                if n > 0 {
                    self.call(wrapper, "SetMaxLines", n, dbg);
                }
            }
        } else if let Some(mode) = level.attr("insertMode").filter(|m| !m.is_empty()) {
            // BOTTOM when it equals "BOTTOM" whole and without case, and TOP otherwise, never a
            // raise (`0x785a46`-`0x785a68`).
            let mode = if mode.eq_ignore_ascii_case("BOTTOM") {
                "BOTTOM"
            } else {
                "TOP"
            };
            self.call(wrapper, "SetInsertMode", mode, dbg);
        }
        for child in &level.children {
            if child.tag.eq_ignore_ascii_case("FontString") {
                self.text_font_child(child, false, wrapper, dbg);
            } else if child.tag.eq_ignore_ascii_case("TextInsets") {
                // The inset read, set when it succeeds (`0x785b49`-`0x785b65`,
                // `0x787d52`-`0x787d6e`).
                if let Some(insets) = self.inset(child) {
                    if let Err(e) = crate::script::set_message_xml_insets(self.lua, wrapper, insets)
                    {
                        self.report.fault(format!("{dbg}: <TextInsets>: {e}"));
                    }
                }
            }
        }
    }

    /// `<ColorSelect>`'s part of one level (`0x78b3f0`): one child loop in document order over its
    /// four textures (`0x78b580`, `0x78b850`, `0x78b8a0`, `0x78ba90`), the hue wheel, the value
    /// strip and a thumb for each. The client generates the wheel and the strip, so a file-less
    /// element still creates its region, for layout, hit tests and paint.
    fn colorselect_level(&mut self, level: &Element, wrapper: &Table, self_name: &str, dbg: &str) {
        for child in &level.children {
            let Some((tag, draw)) = [
                ("ColorWheelTexture", "ARTWORK"),
                ("ColorWheelThumbTexture", "OVERLAY"),
                ("ColorValueTexture", "ARTWORK"),
                ("ColorValueThumbTexture", "OVERLAY"),
            ]
            .into_iter()
            .find(|(t, _)| child.tag.eq_ignore_ascii_case(t)) else {
                continue;
            };
            // The element has no `drawLayer` (`0x78b3f0` never reads it): the slot links its
            // texture on its own layer. The wheel and the strip are painted by the client
            // whatever file or colour the element names (`0x770300`).
            if let Some(tex) = self.build_texture_child(child, wrapper, self_name, draw, dbg) {
                self.install_texture(wrapper, tag, &tex, None, dbg);
            }
        }
    }

    /// `<SimpleHTML>`'s part of one level (`0x78a130`): `font=` links all four element fonts (`0x78a17a`-`0x78a18f`)
    /// or is logged as a miss (`0x78a193`-`0x78a1d0`), then each `<FontString>` and
    /// `<FontStringHeader1..3>` child loads into its element font with the Font loader
    /// (`0x78a1fe`-`0x78a26e` → `0x783c30`), with no template walk. Then `file=`, a document whose
    /// whole text is set as the frame's (`0x78a2a0` loads it through Storm, the attribute
    /// verbatim; `0x78a2b2` sets it), and only after it `hyperlinkFormat` (`0x78a2d4`), so the
    /// document parses with the format the frame held before. A file that does not open leaves
    /// the frame empty.
    fn simplehtml_level(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        let fonts = match crate::script::simplehtml_element_fonts_lua(self.lua, wrapper) {
            Ok(fonts) => fonts,
            Err(e) => {
                self.report.fault(format!("{dbg}: <SimpleHTML> fonts: {e}"));
                return;
            }
        };
        if let Some(font) = el.attr("font").filter(|n| !n.is_empty()) {
            let to = self.model().fonts.id(font);
            match to {
                Some(to) => {
                    for id in fonts {
                        self.link_font_node(id, to, "<unnamed>");
                    }
                }
                None => self.report_font_miss(el, wrapper, font),
            }
        }
        for child in &el.children {
            if let Some(elem) = crate::script::simplehtml_element_of_xml_tag(&child.tag) {
                self.load_font_node(child, fonts[elem], "<unnamed>");
            }
        }
        if let Some(raw) = el.attr("file") {
            if let Some(bytes) = (self.files)(raw) {
                let text = crate::source::decode(&bytes).into_owned();
                self.call(wrapper, "SetText", text, dbg);
            }
        }
        if let Some(fmt) = el.attr("hyperlinkFormat") {
            self.call(wrapper, "SetHyperlinkFormat", fmt.to_string(), dbg);
        }
    }

    /// A model pane's part of one level (`CSimpleModel::LoadXML 0x76cac0`, after the base load at
    /// `0x76cad0`), which `ModelFFX` and the game-side PlayerModel, DressUpModel and TabardModel
    /// share: `file=`, `scale=`, `fogNear` and `fogFar`, each skipped when absent or empty, then
    /// each `<FogColor>` in document order. The base load reads no `scale` (`0x769820`), so a plain
    /// frame's is ignored.
    fn model_level(&mut self, level: &Element, wrapper: &Table, dbg: &str) {
        // `file=` as written is `SetModel` (slot `0x94`, `0x76caf2`), no global-string lookup. A
        // pane left with no model logs `0x878904` at severity 1 (`0x76cb01`-`0x76cb2d`), which the
        // error cube's stand-in for a file that does not load (`0x70737f`) keeps for a load that
        // fails outright.
        if let Some(file) = level.attr("file").filter(|f| !f.is_empty()) {
            self.call(wrapper, "SetModel", file.to_string(), dbg);
            let held = wrapper
                .call_method::<Option<String>>("GetModel", ())
                .ok()
                .flatten()
                .is_some_and(|m| !m.is_empty());
            if !held {
                let name = self.object_name(wrapper);
                self.status_warning(format!("Frame {name}: Bad model file: {file}"));
            }
        }
        // `scale=` through `SStrToFloat` is the model's scale, the field `SetModelScale` writes
        // (`0x76cb61`); a value of 0 or less is logged (`0x8788d8`), never clamped (`0x76cb92`).
        if let Some(scale) = level.attr("scale").filter(|v| !v.is_empty()) {
            let s = s_str_to_float(scale) as f32;
            if s > 0.0 {
                self.call(wrapper, "SetModelScale", s, dbg);
            } else {
                let name = self.object_name(wrapper);
                self.status_warning(format!("Frame {name}: Invalid model scale: {scale}"));
            }
        }
        // The fog: `fogNear`/`fogFar` through `SStrToFloat`, clamped at 0 here only (`0x76cbbb`,
        // `0x76cbf3`), where the Lua setters store raw; each `<FogColor>` in the child loop
        // (`0x76cc20`-`0x76cc5d`), the last winning, is read as a `<Color>` (`0x6f23d0`,
        // `0x76cc43`) and set with `SetFogColor`, fog bit included.
        for (attr, verb) in [("fogNear", "SetFogNear"), ("fogFar", "SetFogFar")] {
            if let Some(v) = level.attr(attr).filter(|v| !v.is_empty()) {
                let v = s_str_to_float(v) as f32;
                self.call(wrapper, verb, v.max(0.0), dbg);
            }
        }
        for fc in children_named(level, "FogColor") {
            self.call(wrapper, "SetFogColor", color_of(fc).fractions(), dbg);
        }
    }
}
