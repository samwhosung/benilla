use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::script::LabelFont;

use super::regions::FontAttrs;
use super::{abs_dim, abs_value, children_named, children_named_any, color_of, Loader};

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
            let expanded = self.expand(child);
            let Some(child_wrapper) = self.materialize(&expanded, Some(wrapper), self_name) else {
                continue; // materialize already reported why
            };
            if let Err(e) = wrapper.call_method::<()>("SetScrollChild", child_wrapper) {
                self.report.fault(format!("{dbg}: <ScrollChild>: {e}"));
            }
        }
    }

    /// `<StatusBar>` (`0x782ef0`): a reversed `minValue`/`maxValue` pair is swapped, as
    /// `SetMinMaxValues` also does; `drawLayer` is the bar texture's layer, ARTWORK by default.
    pub(super) fn apply_statusbar(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        if !el.tag.eq_ignore_ascii_case("StatusBar") {
            return;
        }
        let parse = |attr: &str| el.attr(attr).and_then(|v| v.trim().parse::<f32>().ok());
        let (min, max) = (parse("minValue"), parse("maxValue"));
        if min.is_some() || max.is_some() {
            self.call(
                wrapper,
                "SetMinMaxValues",
                (min.unwrap_or(0.0), max.unwrap_or(0.0)),
                dbg,
            );
        }
        if let Some(o) = el.attr("orientation") {
            self.call(wrapper, "SetOrientation", o.to_string(), dbg);
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
            let c = color_of(bc);
            self.call(wrapper, "SetStatusBarColor", (c[0], c[1], c[2], c[3]), dbg);
        }
        if let Some(v) = parse("defaultValue") {
            self.call(wrapper, "SetValue", v, dbg);
        }
    }

    /// `<Button>`, `<CheckButton>` and `<LootButton>` (`0x7788c0`, which `0x785170` runs first):
    /// the label, the state textures with their own layout, `checked`, `text=` and the per-state
    /// fonts. `$parent` in a child's anchors is this button.
    pub(super) fn apply_button(
        &mut self,
        el: &Element,
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
            // `text=` is a global-string key (`0x703bf0`).
            let label = match bt.attr("text") {
                Some(raw) => self.resolve_text(raw, dbg),
                None => String::new(),
            };
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
                self.call_region(&region, "ClearAllPoints", (), dbg);
                self.apply_region_layout(bt, &region, self_name, dbg, font_attrs);
                // `<ButtonText>` reads its own `inherits=` and `font=` as any FontString does
                // (`0x6f2780` → `0x770f40`); `<NormalText>` skips them (`0x778b7b`).
                if font_attrs == FontAttrs::Own {
                    self.apply_fontstring_font(bt, &region, dbg);
                }
                self.apply_wrap_attrs(bt, &region, dbg);
                if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region) {
                    self.report.fault(format!("{dbg}: implicit anchor: {e}"));
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
        if let Some(text) = el.attr("text") {
            // A global-string key (`0x703bf0`): `text="DELETE"` reads "Delete".
            let text = self.resolve_text(text, dbg);
            self.call(wrapper, "SetText", text, dbg);
        }
        // The per-state fonts, two spellings each; every occurrence applies, in document order.
        for (children, method, which) in [
            (
                ["NormalFont", "NormalText"],
                "SetTextFontObject",
                LabelFont::Normal,
            ),
            (
                ["HighlightFont", "HighlightText"],
                "SetHighlightFontObject",
                LabelFont::Highlight,
            ),
            (
                ["DisabledFont", "DisabledText"],
                "SetDisabledFontObject",
                LabelFont::Disabled,
            ),
        ] {
            for f in children_named_any(el, &children) {
                // `<Font>`-typed elements (`0x778bf4` → `0x783c30`): `inherits=` and `font=` land
                // in one slot and `font=` wins (`0x783d22`). `style=` is not 1.12 and is not read.
                for attr in ["inherits", "font"] {
                    if let Some(name) = f.attr(attr) {
                        self.call(wrapper, method, name.to_string(), dbg);
                    }
                }
                // A `justifyH` here is a local write on that state's font, never the label
                // (`0x783c30`); the button reads it to anchor a label it makes later (`0x778d20`).
                if let Some(j) = f.attr("justifyH") {
                    match crate::justify::parse_h(j) {
                        crate::justify::Set::To(jh) => {
                            if let Err(e) = crate::script::set_label_font_justify_h_lua(
                                self.lua, wrapper, which, jh,
                            ) {
                                self.report
                                    .fault(format!("{dbg}: <{}> justifyH: {e}", f.tag));
                            }
                        }
                        // Reported, not modelled: the reference clears the axis on a cross-axis
                        // token and raises on a non-token.
                        _ => self.report.warnings.push(format!(
                            "{dbg}: <{}> justifyH=\"{j}\" is not a horizontal token — ignored",
                            f.tag
                        )),
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
        if let Some(n) = el
            .attr("letters")
            .and_then(|v| v.trim().parse::<i64>().ok())
        {
            self.call(wrapper, "SetMaxLetters", n, dbg);
        }
        if let Some(n) = el
            .attr("historyLines")
            .and_then(|v| v.trim().parse::<i64>().ok())
        {
            self.call(wrapper, "SetHistoryLines", n, dbg);
        }
        if let Some(s) = el
            .attr("blinkSpeed")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            self.call(wrapper, "SetBlinkSpeed", s, dbg);
        }
        if let Some(ins) = children_named(el, "TextInsets").next() {
            let src = children_named(ins, "AbsInset").next().unwrap_or(ins);
            let get = |k: &str| {
                src.attr(k)
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            let (l, r, t, b) = (get("left"), get("right"), get("top"), get("bottom"));
            self.call(wrapper, "SetTextInsets", (l, r, t, b), dbg);
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
    /// the stock model when the attribute is absent, as the reference reads a default string.
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
        if let Err(e) = crate::script::apply_minimap_model_attrs(self.lua, wrapper, arrow, player) {
            self.warn_once(
                "minimap:models",
                format!("{dbg}: <Minimap> model attributes: {e}"),
            );
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
        if scrolling {
            if let Some(n) = el
                .attr("maxLines")
                .and_then(|v| v.trim().parse::<i64>().ok())
            {
                if n > 0 {
                    self.call(wrapper, "SetMaxLines", n, dbg);
                }
            }
        }
        if plain {
            if let Some(mode) = el.attr("insertMode") {
                self.call(wrapper, "SetInsertMode", mode.trim().to_string(), dbg);
            }
        }
        if let Some(s) = el
            .attr("displayDuration")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            if s > 0.0 {
                self.call(wrapper, "SetTimeVisible", s, dbg);
            }
        }
        if let Some(s) = el
            .attr("fadeDuration")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            if s > 0.0 {
                self.call(wrapper, "SetFadeDuration", s, dbg);
            }
        }
        // `0x6f1b30`, a missing or empty one skipped (`0x7859b2`-`0x7859bd`).
        if let Some(on) = el.attr_bool_opt("fade") {
            self.call(wrapper, "SetFading", on, dbg);
        }
    }

    /// `<Slider>` (`0x789580`), VERTICAL from the ctor: the range, `valueStep` and `defaultValue`
    /// apply only when both bounds are present, and a reversed pair is not swapped; `drawLayer` is
    /// the thumb's layer, OVERLAY by default.
    pub(super) fn apply_slider(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        if !el.tag.eq_ignore_ascii_case("Slider") {
            return;
        }
        if let Some(o) = el.attr("orientation") {
            self.call(wrapper, "SetOrientation", o.to_string(), dbg);
        }
        let parse = |attr: &str| el.attr(attr).and_then(|v| v.trim().parse::<f32>().ok());
        if let (Some(min), Some(max)) = (parse("minValue"), parse("maxValue")) {
            self.call(wrapper, "SetMinMaxValues", (min, max), dbg);
            if let Some(step) = parse("valueStep") {
                self.call(wrapper, "SetValueStep", step, dbg);
            }
            if let Some(v) = parse("defaultValue") {
                self.call(wrapper, "SetValue", v, dbg);
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

    /// `<SimpleHTML>` (`0x78a130`): `font=` for all four element fonts (`0x78a17a`), the
    /// `<FontString>` and `<FontStringHeader1..3>` children one each (`0x78a1fe`, through
    /// `0x783c30`), `hyperlinkFormat` (`0x78a540`), and `file=`, a document whose whole text is
    /// set as the frame's (`0x78a2a0` loads it through Storm, the attribute verbatim; `0x78a2b2`
    /// sets it). A file that does not open leaves the frame empty.
    pub(super) fn apply_simplehtml(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if !el.tag.eq_ignore_ascii_case("SimpleHTML") {
            return;
        }
        if let Some(fmt) = el.attr("hyperlinkFormat") {
            self.call(wrapper, "SetHyperlinkFormat", fmt.to_string(), dbg);
        }
        // Read before the children (`0x78a152`), which may override it.
        if let Some(name) = el.attr("font").filter(|n| !n.is_empty()) {
            if self.is_font_object(name) {
                for elem in ["P", "H1", "H2", "H3"] {
                    self.call(wrapper, "SetFontObject", (elem, name.to_string()), dbg);
                }
            } else {
                self.warn_once(
                    &format!("shtmlfont:{name}"),
                    format!("{dbg}: <SimpleHTML font=\"{name}\">: couldn't find font object"),
                );
            }
        }
        for child in &el.children {
            let Some(elem) = crate::script::simplehtml_element_of_xml_tag(&child.tag) else {
                continue;
            };
            let child = &self.expand_region(child);
            self.apply_element_font(child, wrapper, elem, dbg);
        }
        if let Some(raw) = el.attr("file") {
            if let Some(bytes) = (self.files)(raw) {
                let text = crate::source::decode(&bytes).into_owned();
                self.call(wrapper, "SetText", text, dbg);
            }
        }
    }

    /// One `<SimpleHTML>` font child's element font, with the same `font=` gate as
    /// [`Self::apply_fontstring_font`].
    fn apply_element_font(&mut self, el: &Element, wrapper: &Table, elem: usize, dbg: &str) {
        let name = ["P", "H1", "H2", "H3"][elem];
        if let Some(inherits) = el.attr("inherits").filter(|n| !n.is_empty()) {
            let resolved = self
                .font_object_through_templates(inherits)
                .unwrap_or_else(|| inherits.to_string());
            self.call(wrapper, "SetFontObject", (name, resolved), dbg);
        }
        match el.attr("font") {
            Some(f) if self.is_font_object(f) => {
                self.call(wrapper, "SetFontObject", (name, f.to_string()), dbg);
            }
            Some(path) => {
                let height = children_named(el, "FontHeight").last().and_then(abs_value);
                let outline = el.attr("outline").map(str::to_string);
                if let Err(e) = crate::script::apply_simplehtml_font_parts(
                    self.lua,
                    wrapper,
                    elem,
                    Some(path.to_string()),
                    height,
                    outline,
                ) {
                    self.report
                        .fault(format!("{dbg}: <SimpleHTML> {name} font attrs: {e}"));
                }
            }
            None => {}
        }
        if let Some(c) = children_named(el, "Color").last().map(color_of) {
            self.call(wrapper, "SetTextColor", (name, c[0], c[1], c[2], c[3]), dbg);
        }
        if let Some(sh) = children_named(el, "Shadow").last() {
            if let Some(c) = children_named(sh, "Color").next().map(color_of) {
                self.call(
                    wrapper,
                    "SetShadowColor",
                    (name, c[0], c[1], c[2], c[3]),
                    dbg,
                );
            }
            if let Some((x, y)) = children_named(sh, "Offset").next().map(abs_dim) {
                self.call(
                    wrapper,
                    "SetShadowOffset",
                    (name, x.unwrap_or(0.0), y.unwrap_or(0.0)),
                    dbg,
                );
            }
        }
        if let Some(j) = el.attr("justifyH") {
            self.call(wrapper, "SetJustifyH", (name, j.to_string()), dbg);
        }
        if let Some(j) = el.attr("justifyV") {
            self.call(wrapper, "SetJustifyV", (name, j.to_string()), dbg);
        }
        if let Some(sp) = el
            .attr("spacing")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            self.call(wrapper, "SetSpacing", (name, sp), dbg);
        }
    }
}
