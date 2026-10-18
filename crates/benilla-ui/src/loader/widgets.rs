use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::script::binding_abi::s_str_to_int;
use crate::script::LabelFont;

use super::regions::FontAttrs;
use super::{children_named, children_named_any, color_of, Levels, Loader};

impl Loader<'_> {
    /// `<ScrollFrame>`'s `<ScrollChild>` (`LoadXML 0x786bc0`): after the base load (`0x769820`) it
    /// builds the node's one child through `Instantiate 0x6ee280` (`0x786bf4`), a whole frame
    /// lifecycle with the child's own `<Frames>` and OnLoad, and only then stores it with
    /// `SetScrollChild` (`0x786ce0`), which marks the frame's scroll range. So the child's OnLoad
    /// sees no scroll child yet. A template's `<ScrollChild>` was built by its own `LoadXML` level
    /// (`0x76985c`), ahead of the instance's, which therefore wins. An empty one errors in the
    /// reference (`0x786c0c`). Other frame kinds do not read the element.
    pub(super) fn apply_scrollframe(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        if !el.tag.eq_ignore_ascii_case("ScrollFrame") {
            return;
        }
        for sc in children_named(el, "ScrollChild") {
            let Some(child) = sc.children.first() else {
                self.report.fault(format!(
                    "{dbg}: <ScrollChild> is empty — the reference errors here, and a \
                     ScrollFrame with no child has no scroll range at all"
                ));
                continue;
            };
            let Some(child_wrapper) = self.materialize(child, Some(wrapper), self_name) else {
                continue; // materialize already reported why
            };
            if let Err(e) = wrapper.call_method::<()>("SetScrollChild", child_wrapper) {
                self.report.fault(format!("{dbg}: <ScrollChild>: {e}"));
            }
        }
    }

    /// `<StatusBar>` (`0x782ef0`): the `<BarTexture>`, built on `drawLayer` (ARTWORK by default),
    /// and `<BarColor>`, then for each level ([`Loader::levels`]) its range, its value and its
    /// orientation.
    pub(super) fn apply_statusbar(
        &mut self,
        el: &Element,
        levels: &Levels,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        if !el.tag.eq_ignore_ascii_case("StatusBar") {
            return;
        }
        let layer = el.attr("drawLayer");
        for bar in children_named(el, "BarTexture") {
            // Built as any texture element is, and installed as the bar on `drawLayer`, ARTWORK
            // without one (`0x782f12`); `<BarColor>` is the bar's tint.
            let level = layer.unwrap_or("ARTWORK");
            if let Some(tex) = self.build_texture_child(bar, wrapper, self_name, level, dbg) {
                self.install_texture(wrapper, "BarTexture", &tex, layer, dbg);
            }
        }
        for bc in children_named(el, "BarColor") {
            self.call(wrapper, "SetStatusBarColor", color_of(bc).fractions(), dbg);
        }
        for level in levels {
            // Each value with `SStrToFloat`, present and non-empty: the range needs both bounds on the
            // node, and `defaultValue` is read only after it (`0x782fb0`-`0x783021`). The setter
            // `0x783270` clamps a min above the max down to it.
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
    }

    /// `<Button>`, `<CheckButton>` and `<LootButton>` (`0x7788c0`, which `0x785170` runs first):
    /// the label, the state textures with their own layout, `checked`, `text=` and the per-state
    /// fonts. `$parent` in a child's anchors is this button.
    pub(super) fn apply_button(
        &mut self,
        el: &Element,
        levels: &Levels,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        let is_check = el.tag.eq_ignore_ascii_case("CheckButton");
        // A `<LootButton>` loads exactly as a Button: its `LoadXML` slot holds `CSimpleButton`'s
        // pointer (`0x804594[2]` = `0x81c7c8[2]`).
        if !is_check
            && !el.tag.eq_ignore_ascii_case("Button")
            && !el.tag.eq_ignore_ascii_case("LootButton")
        {
            return;
        }
        // Each state-texture element is a new region from the `<Layers>` texture adder (`0x6f26f0`,
        // `0x778903`): named, laid out, painted and given its implicit all-points as any layer
        // texture is, then handed to the slot store, which destroys the region it displaces. An
        // instance's element over its template's therefore starts fresh: `LoadXML` runs the
        // template's level first (`0x76985c`), and nothing of that region survives the instance's.
        // The Lua path leg differs: it repaints the slot's region and keeps its state (`0x778f10`).
        let tex = |tag: &str, this: &mut Self| {
            for raw in children_named(el, tag) {
                let Some(region) =
                    this.build_texture_child(raw, wrapper, self_name, "ARTWORK", dbg)
                else {
                    continue; // build_texture_child already reported why
                };
                this.install_texture(wrapper, tag, &region, None, dbg);
                // Freeing the displaced region took its entry out of the name lookup, which a
                // replacement of the same name takes back. The replacement's `GetName()` is its
                // own (`0x76c650`); `_G` keeps its first holder (`0x701c8a`).
                if let Some(rname) = raw.name().map(|n| framexml::resolve_name(n, self_name)) {
                    crate::script::region::publish_region_name(this.lua(), &rname, &region);
                }
            }
        };
        // Both label spellings build the label, in document order: `<ButtonText>` through the
        // ordinary FontString builder (`0x7789d0` → `0x6f2780`), `<NormalText>` through an inline
        // copy that disowns its font attributes (`0x778b7b`). `<HighlightText>` and
        // `<DisabledText>` build no region, only fonts (`0x778bf4`).
        for bt in children_named_any(el, &["ButtonText", "NormalText"]) {
            let font_attrs = if bt.tag.eq_ignore_ascii_case("NormalText") {
                FontAttrs::Disowned
            } else {
                FontAttrs::Own
            };
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
            // A named label is created as a named region, not aliased after `SetText`, so its
            // `GetName()` answers.
            let bt_name = bt.name().map(|raw| framexml::resolve_name(raw, self_name));
            if let Some(rname) = bt_name.clone() {
                match wrapper
                    .call_method::<Table>("CreateFontString", (rname, Option::<String>::None))
                {
                    Ok(fs) => self.call(wrapper, "SetFontString", fs, dbg),
                    Err(e) => self
                        .report
                        .warnings
                        .push(format!("{dbg}: ButtonText CreateFontString: {e}")),
                }
            }
            self.call(wrapper, "SetText", label, dbg);
            if let Ok(region) = wrapper.call_method::<Table>("GetFontString", ()) {
                // Clear, lay out, then the implicit step, as both reference legs do (`0x6f27f5`,
                // `0x778b96`); the button's own adopter anchor never fires on them (`0x778d5f`).
                // On `<NormalText>` the justify is the Normal font's, so the implicit anchor reads
                // the ctor's CENTER (`0x212`); the justify still reaches the paint (`0x784111`).
                // Both load as any FontString does (`0x6f2780` → `0x770f40`, `0x778b8f`), each
                // level in turn; `<NormalText>` skips its font attributes (`0x778b7b`).
                self.call_region(&region, "ClearAllPoints", (), dbg);
                for level in &label_levels {
                    self.load_fontstring_level(level, &region, self_name, font_attrs, false, dbg);
                }
                if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region) {
                    self.report.fault(format!("{dbg}: implicit anchor: {e}"));
                }
                // Then the install's refresh (`0x778daa`): the label follows the state's font,
                // not the font its own `inherits=` or `font=` linked.
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
        }
        tex("NormalTexture", self);
        tex("PushedTexture", self);
        tex("DisabledTexture", self);
        tex("HighlightTexture", self);
        if is_check {
            tex("CheckedTexture", self);
            tex("DisabledCheckedTexture", self);
            // Read through `0x6f1b30`, a missing or empty one skipped (`0x78518e`-`0x78519d`).
            if let Some(checked) = el.attr_bool_opt("checked") {
                self.call(wrapper, "SetChecked", checked, dbg);
            }
        }
        // Each level's `text=`, a global-string key (`0x703bf0`, `text="DELETE"` reads "Delete"),
        // an empty one skipped (`0x778c1a`).
        for level in levels {
            if let Some(text) = level.attr("text").filter(|t| !t.is_empty()) {
                let text = self.resolve_text(text, dbg);
                self.call(wrapper, "SetText", text, dbg);
            }
        }
        // `<PushedTextOffset>` (`0x778aec`): the dimension read (`0x6f1eb0`), stored as
        // `SetPushedTextOffset` does (`0x778b24`) only when it succeeds (`0x778b0c`).
        for el in children_named(el, "PushedTextOffset") {
            let d = self.dimension(el);
            if d.ok {
                let [x, y] = d.script(self.model().layout_units());
                self.call(wrapper, "SetPushedTextOffset", (x, y), dbg);
                crate::script::xml_layout::pushed_text_offset(self.lua, wrapper, d.v);
            }
        }
        // The three embedded fonts and their colours, in document order (`0x7788c0`): each font
        // element, two spellings each, loads into its font as a `<Font>` does (`0x778baf`,
        // `0x778bf4` → `0x783c30`); each colour element (`0x6f23d0`) is that font's text colour.
        // Deviation: `<HighlightColor>` and `<DisabledColor>` colour their own fonts; 1.12.1 sends
        // both to the normal colour setter (`0x778aa4`, `0x778ada` call `0x778e20`), so neither
        // could reach its state.
        for child in &el.children {
            let tag = child.tag.as_str();
            let font = [
                ("NormalFont", LabelFont::Normal),
                ("NormalText", LabelFont::Normal),
                ("HighlightFont", LabelFont::Highlight),
                ("HighlightText", LabelFont::Highlight),
                ("DisabledFont", LabelFont::Disabled),
                ("DisabledText", LabelFont::Disabled),
            ]
            .into_iter()
            .find(|(t, _)| tag.eq_ignore_ascii_case(t));
            if let Some((_, which)) = font {
                match crate::script::state_font_lua(self.lua, wrapper, which) {
                    Ok(id) => self.load_font_node(child, id, "<unnamed>"),
                    Err(e) => self.report.fault(format!("{dbg}: <{tag}>: {e}")),
                }
                continue;
            }
            let color = [
                ("NormalColor", "SetTextColor"),
                ("HighlightColor", "SetHighlightTextColor"),
                ("DisabledColor", "SetDisabledTextColor"),
            ]
            .into_iter()
            .find(|(t, _)| tag.eq_ignore_ascii_case(t));
            if let Some((_, method)) = color {
                self.call(wrapper, method, color_of(child).fractions(), dbg);
            }
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

    /// An EditBox's, a MessageFrame's or a ScrollingMessageFrame's `<FontString>` children, each
    /// loaded into the frame's own font by the Font loader (`0x783c30` at `0x77a220`, `0x785af5`,
    /// `0x787cfe`), never a region: one over a font that already follows another first logs
    /// `%s: FontString element overriding font %s` (`0x879b3c`) with the frame's name and the
    /// followed font's. An EditBox's then caps its bytes (`0x77a225`).
    pub(super) fn apply_text_font_children(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        for child in children_named(el, "FontString") {
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
            if el.tag.eq_ignore_ascii_case("EditBox") {
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
            }
        }
    }

    /// `<EditBox>` (`0x779fb0`). An absent flag keeps the ctor's, where `autoFocus` is on and the
    /// rest off (`0x779a29`, `0x77a0b3`), so flags are read presence-aware: `autoFocus="false"`
    /// clears. The reference also skips an empty `autoFocus` (`0x77a0b8`), which clears here.
    /// `blinkSpeed` is the caret half-period (`E+0x370`).
    pub(super) fn apply_editbox(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if !el.tag.eq_ignore_ascii_case("EditBox") {
            return;
        }
        // Each non-empty: `letters` through `SStrToInt` (`0x77a05c`), `blinkSpeed` through
        // `SStrToFloat` and stored as 0 unless above it (`0x77a07f`-`0x77a09f`), `historyLines`
        // through `SStrToInt` and 0 unless above it (`0x77a175`-`0x77a18d`).
        if let Some(v) = el.attr("letters").filter(|v| !v.is_empty()) {
            self.call(wrapper, "SetMaxLetters", s_str_to_int(v), dbg);
        }
        if let Some(v) = el.attr("historyLines").filter(|v| !v.is_empty()) {
            // The loader's own setter (`0x77cd90`), not the Lua binding's, which refuses 0.
            if let Err(e) = crate::script::set_history_lines(self.lua, wrapper, s_str_to_int(v)) {
                self.report.fault(format!("{dbg}: SetHistoryLines: {e}"));
            }
        }
        if let Some(v) = el.attr("blinkSpeed").filter(|v| !v.is_empty()) {
            let s = super::xml_float(v);
            self.call(wrapper, "SetBlinkSpeed", if s > 0.0 { s } else { 0.0 }, dbg);
        }
        // Each `<TextInsets>` in the child loop (`0x77a2a9`), the last winning, through the inset
        // read (`0x77a2d5`), set when it succeeds (`0x77a2da`).
        for ins in children_named(el, "TextInsets") {
            if let Some(v) = self.inset(ins) {
                let [l, r, t, b] = self.script_of(v);
                self.call(wrapper, "SetTextInsets", (l, r, t, b), dbg);
                crate::script::xml_layout::text_insets(self.lua, wrapper, v);
            }
        }
        // Each `<HighlightColor>` in turn, the last winning (`0x77a277`): the `<Color>` reader
        // (`0x6f23d0`, `0x77a292`), then the selection's setter (`0x77a2a2`).
        for hc in children_named(el, "HighlightColor") {
            if let Err(e) = crate::script::set_selection_color(self.lua, wrapper, color_of(hc)) {
                self.report.fault(format!("{dbg}: <HighlightColor>: {e}"));
            }
        }
        for (attr, method) in [
            ("autoFocus", "SetAutoFocus"),
            ("numeric", "SetNumeric"),
            ("password", "SetPassword"),
            ("multiLine", "SetMultiLine"),
            // One flag behind both spellings (`0x77a6b0`, `0x7996e0`, bit 0x10).
            ("ignoreArrows", "SetAltArrowKeyMode"),
        ] {
            if let Some(on) = el.attr_bool_opt(attr) {
                self.call(wrapper, method, on, dbg);
            }
        }
    }

    /// `<Minimap>` (`0x4ee2b0`): the two model files for the ctor's nine `Model` children, each
    /// the stock model when the attribute is absent, as the reference reads a default string, and
    /// its log line for a model that does not load.
    pub(super) fn apply_minimap(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if !el.tag.eq_ignore_ascii_case("Minimap") {
            return;
        }
        let arrow = el
            .attr("minimapArrowModel")
            .filter(|s| !s.is_empty())
            .unwrap_or(crate::widget::MINIMAP_DEFAULT_ARROW_MODEL);
        let player = el
            .attr("minimapPlayerModel")
            .filter(|s| !s.is_empty())
            .unwrap_or(crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL);
        match crate::script::apply_minimap_model_attrs(self.lua, wrapper, arrow, player) {
            Ok(failed) => {
                for line in failed {
                    self.report.warnings.push(format!("{dbg}: {line}"));
                }
            }
            Err(e) => self.warn_once(
                "minimap:models",
                format!("{dbg}: <Minimap> model attributes: {e}"),
            ),
        }
    }

    /// `<ScrollingMessageFrame>` and `<MessageFrame>`, two loaders (`0x787b20`, `0x785910`): both
    /// take `displayDuration` and `fadeDuration` (only when > 0) and `fade`; only the scrolling one
    /// has `maxLines` (> 0), only the plain one `insertMode`. The line font and its `justifyH` come
    /// from the direct-child `<FontString>`.
    pub(super) fn apply_messageframe(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        let scrolling = el.tag.eq_ignore_ascii_case("ScrollingMessageFrame");
        let plain = el.tag.eq_ignore_ascii_case("MessageFrame");
        if !scrolling && !plain {
            return;
        }
        // A non-empty `maxLines` through `SStrToInt`, above 0 (`0x787c5e`-`0x787c71`).
        if scrolling {
            if let Some(v) = el.attr("maxLines").filter(|v| !v.is_empty()) {
                let n = s_str_to_int(v);
                if n > 0 {
                    self.call(wrapper, "SetMaxLines", n, dbg);
                }
            }
        }
        // A non-empty `insertMode` is BOTTOM when it equals "BOTTOM" whole and without case, and
        // TOP otherwise, never a raise (`0x785a46`-`0x785a68`).
        if plain {
            if let Some(mode) = el.attr("insertMode").filter(|m| !m.is_empty()) {
                let mode = if mode.eq_ignore_ascii_case("BOTTOM") {
                    "BOTTOM"
                } else {
                    "TOP"
                };
                self.call(wrapper, "SetInsertMode", mode, dbg);
            }
        }
        // Each a non-empty value through `SStrToFloat`, above 0 (`0x7859de`, `0x785a15`).
        for (attr, verb) in [
            ("displayDuration", "SetTimeVisible"),
            ("fadeDuration", "SetFadeDuration"),
        ] {
            if let Some(v) = el.attr(attr).filter(|v| !v.is_empty()) {
                let s = super::xml_float(v);
                if s > 0.0 {
                    self.call(wrapper, verb, s, dbg);
                }
            }
        }
        // `0x6f1b30`, a missing or empty one skipped (`0x7859b2`-`0x7859bd`).
        if let Some(on) = el.attr_bool_opt("fade") {
            self.call(wrapper, "SetFading", on, dbg);
        }
        // Each `<TextInsets>` in the child loop, through the inset read, set when it succeeds
        // (`0x785b49`-`0x785b65`, `0x787d52`-`0x787d6e`).
        for ins in children_named(el, "TextInsets") {
            if let Some(insets) = self.inset(ins) {
                if let Err(e) = crate::script::set_message_xml_insets(self.lua, wrapper, insets) {
                    self.report.fault(format!("{dbg}: <TextInsets>: {e}"));
                }
            }
        }
    }

    /// `<Slider>` (`0x789580`), VERTICAL from the ctor. Its frame load (`0x769820`) first re-enters
    /// the virtual `LoadXML` on the `inherits` node (`0x76985c`), so for each level
    /// ([`Loader::levels`]), the template first, the range, `valueStep` and `defaultValue` apply
    /// only when both bounds are on that node, and a reversed pair is not swapped; `drawLayer` is
    /// the thumb's layer, OVERLAY by default.
    pub(super) fn apply_slider(
        &mut self,
        el: &Element,
        levels: &Levels,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        if !el.tag.eq_ignore_ascii_case("Slider") {
            return;
        }
        for level in levels {
            // Each value through `SStrToFloat` (`0x64aaa0`), present and non-empty, so `"100%"`
            // is 100: the range needs both bounds on the node, and `valueStep` and `defaultValue`
            // are read only after it (`0x789605`-`0x7896a2`).
            let value = |attr: &str| {
                level
                    .attr(attr)
                    .filter(|v| !v.is_empty())
                    .map(|v| crate::script::binding_abi::s_str_to_float(v) as f32)
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
            // `orientation` through the token parse `0x6f1a20`: empty is skipped, a miss logged
            // (`0x87a3c4`, `0x78970d`) and never raised, where the Lua setter raises.
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
        let layer = el.attr("drawLayer");
        for raw in children_named(el, "ThumbTexture") {
            // Built as any texture element is (a named thumb is published, which
            // `ScrollFrame_OnScrollRangeChanged` reads by name), then installed as the thumb on
            // `drawLayer`, OVERLAY without one; the install clears its anchors (`0x789893`).
            let level = layer.unwrap_or("OVERLAY");
            if let Some(tex) = self.build_texture_child(raw, wrapper, self_name, level, dbg) {
                self.install_texture(wrapper, "ThumbTexture", &tex, layer, dbg);
            }
        }
    }

    /// `<ColorSelect>`'s four textures (`0x78b580`, `0x78b850`, `0x78b8a0`, `0x78ba90`): the hue
    /// wheel, the value strip and a thumb for each. The client generates the wheel and the strip,
    /// so a file-less element still creates its region, for layout, hit tests and paint.
    pub(super) fn apply_colorselect(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        if !el.tag.eq_ignore_ascii_case("ColorSelect") {
            return;
        }
        for (tag, level) in [
            ("ColorWheelTexture", "ARTWORK"),
            ("ColorWheelThumbTexture", "OVERLAY"),
            ("ColorValueTexture", "ARTWORK"),
            ("ColorValueThumbTexture", "OVERLAY"),
        ] {
            for raw in children_named(el, tag) {
                // The element has no `drawLayer` (`0x78b3f0` never reads it): the slot links its
                // texture on its own layer. The wheel and the strip are painted by the client
                // whatever file or colour the element names (`0x770300`).
                if let Some(tex) = self.build_texture_child(raw, wrapper, self_name, level, dbg) {
                    self.install_texture(wrapper, tag, &tex, None, dbg);
                }
            }
        }
    }

    /// `<SimpleHTML>` (`0x78a130`): `font=` links all four element fonts (`0x78a17a`-`0x78a18f`)
    /// or is logged as a miss (`0x78a193`-`0x78a1d0`), then each `<FontString>` and
    /// `<FontStringHeader1..3>` child loads into its element font with the Font loader
    /// (`0x78a1fe`-`0x78a26e` → `0x783c30`), with no template walk. Then `file=`, a document whose
    /// whole text is set as the frame's (`0x78a2a0` loads it through Storm, the attribute
    /// verbatim; `0x78a2b2` sets it), and only after it `hyperlinkFormat` (`0x78a2d4`), so the
    /// document parses with the format the frame held before. A file that does not open leaves
    /// the frame empty.
    pub(super) fn apply_simplehtml(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if !el.tag.eq_ignore_ascii_case("SimpleHTML") {
            return;
        }
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
}
