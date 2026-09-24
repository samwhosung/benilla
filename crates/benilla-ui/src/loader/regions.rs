use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};

use super::{abs_dim, abs_value, children_named, color_of, tex_coords_of, Loader};

/// Whether a region reads its own font attributes, the reference's `FONTSTRING+0x12c`: set by the
/// ctor (`0x770de7`) and cleared only on a Button's `<NormalText>` (`0x778b7b`), whose string then
/// skips them (`0x7710e1`-`0x771467`) for the button's Normal font to take (`0x783c30`). Here it
/// gates only justify, the one font attribute the button's label pass applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FontAttrs {
    /// Every other region, a Button's `<ButtonText>` included.
    Own,
    /// A Button's `<NormalText>`: its font attributes are the button's Normal font's.
    Disowned,
}

impl Loader<'_> {
    /// `<Layers>` (`0x769d70`): each `<Texture>` and `<FontString>`, created on its layer.
    pub(super) fn apply_layers(
        &mut self,
        el: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        for layers in children_named(el, "Layers") {
            for layer in children_named(layers, "Layer") {
                let level = layer.attr("level").unwrap_or("ARTWORK").to_string();
                for region in &layer.children {
                    // `inherits=` may name a region template, in the frames' one registry
                    // (`0x6ee500`); a font object's name passes on to `apply_fontstring_font`.
                    let region = &self.expand_region(region);
                    let is_texture = region.tag.eq_ignore_ascii_case("Texture");
                    let is_fontstring = region.tag.eq_ignore_ascii_case("FontString");
                    if !is_texture && !is_fontstring {
                        self.report.warnings.push(format!(
                            "{dbg}: <Layer> child <{}> is not a Texture/FontString; skipped",
                            region.tag
                        ));
                        continue;
                    }
                    let rname: Option<String> = region
                        .name()
                        .map(|raw| framexml::resolve_name(raw, parent_name));
                    let method = if is_texture {
                        "CreateTexture"
                    } else {
                        "CreateFontString"
                    };
                    let region_wrapper: Table =
                        match wrapper.call_method(method, (rname, Some(level.clone()))) {
                            Ok(w) => w,
                            Err(e) => {
                                self.report.errors.push(format!("{dbg}: {method}: {e}"));
                                continue;
                            }
                        };
                    if region.attr_bool("hidden") {
                        if let Err(e) = region_wrapper.call_method::<()>("Hide", ()) {
                            self.report.errors.push(format!("{dbg}: region Hide: {e}"));
                        }
                    }
                    if let Some(a) = region
                        .attr("alpha")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                    {
                        self.call_region(&region_wrapper, "SetAlpha", a, dbg);
                    }
                    self.apply_region_layout(
                        region,
                        &region_wrapper,
                        parent_name,
                        dbg,
                        FontAttrs::Own,
                    );
                    if is_fontstring {
                        // Before the visual pass, so an explicit `<Color>` beats the font object's.
                        self.apply_fontstring_font(region, &region_wrapper, dbg);
                    }
                    self.apply_region_visual(region, &region_wrapper, is_texture, dbg);
                    // The implicit creation anchor, run after the region's LoadXML as the
                    // reference does (`0x7701c0`, `0x771480`); an anchored region is left alone.
                    if let Err(e) =
                        crate::script::implicit_creation_anchor_lua(self.lua, &region_wrapper)
                    {
                        self.report
                            .errors
                            .push(format!("{dbg}: implicit anchor: {e}"));
                    }
                }
            }
        }
    }

    /// A frame's direct-child `<FontString>`, its "special" string (a message frame's line font, an
    /// EditBox's text), on the OVERLAY layer.
    pub(super) fn apply_special_fontstrings(
        &mut self,
        el: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        // A `<SimpleHTML>`'s direct `<FontString>` declares its `P` font, not a region
        // (`0x78a1fe`); `apply_simplehtml` owns it.
        if el.tag.eq_ignore_ascii_case("SimpleHTML") {
            return;
        }
        for region in &el.children {
            if !region.tag.eq_ignore_ascii_case("FontString") {
                continue;
            }
            let region = &self.expand_region(region);
            let rname: Option<String> = region
                .name()
                .map(|raw| framexml::resolve_name(raw, parent_name));
            // An EditBox's `<FontString>` declares the text string its ctor built (`0x779fb0`,
            // `0x779bee`), so it creates nothing: a second region would also shift `GetRegions`.
            let existing_text = el
                .tag
                .eq_ignore_ascii_case("EditBox")
                .then(|| crate::script::editbox_text_region_wrapper(self.lua(), wrapper))
                .flatten();
            let region_wrapper: Table = match existing_text {
                Some(w) => {
                    if let Some(n) = rname {
                        crate::script::region::publish_region_name(self.lua(), &n, &w);
                        if let Err(e) = self.lua().globals().set(n.clone(), w.clone()) {
                            self.report
                                .warnings
                                .push(format!("{dbg}: embedded FontString global '{n}': {e}"));
                        }
                    }
                    w
                }
                None => match wrapper
                    .call_method("CreateFontString", (rname, Some("OVERLAY".to_string())))
                {
                    Ok(w) => w,
                    Err(e) => {
                        self.report
                            .errors
                            .push(format!("{dbg}: CreateFontString (special): {e}"));
                        continue;
                    }
                },
            };
            self.apply_region_layout(region, &region_wrapper, parent_name, dbg, FontAttrs::Own);
            self.apply_fontstring_font(region, &region_wrapper, dbg);
            self.apply_region_visual(region, &region_wrapper, false, dbg);
            // The implicit creation anchor, as in `apply_layers`; an EditBox's adopt replaces it.
            if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region_wrapper) {
                self.report
                    .errors
                    .push(format!("{dbg}: implicit anchor: {e}"));
            }
            // It is the EditBox's text region by slot, never found by a search, which could take a
            // `<Layers>` string; the box's insets anchor it.
            if el.tag.eq_ignore_ascii_case("EditBox") {
                if let Err(e) =
                    crate::script::adopt_text_region(self.lua(), wrapper, &region_wrapper)
                {
                    self.report
                        .errors
                        .push(format!("{dbg}: adopt special FontString: {e}"));
                }
            }
        }
    }

    /// A region's geometry: `<Size>`, a FontString's justify when [`FontAttrs::Own`],
    /// `setAllPoints` and `<Anchors>`, whose `$parent` is the owner's name, or a nameless owner's
    /// nearest named ancestor's.
    pub(super) fn apply_region_layout(
        &mut self,
        region: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
        font_attrs: FontAttrs,
    ) {
        // Every `<Size>` in document order, as in `apply_size`.
        for size in children_named(region, "Size") {
            let (x, y) = abs_dim(size);
            if let Some(w) = x {
                self.call_region(wrapper, "SetWidth", w, dbg);
            }
            if let Some(h) = y {
                self.call_region(wrapper, "SetHeight", h, dbg);
            }
        }
        if font_attrs == FontAttrs::Own {
            if let Some(j) = region.attr("justifyH") {
                self.call_region(wrapper, "SetJustifyH", j.to_string(), dbg);
            }
            if let Some(j) = region.attr("justifyV") {
                self.call_region(wrapper, "SetJustifyV", j.to_string(), dbg);
            }
        }
        if region.attr_bool("setAllPoints") {
            self.call_region(wrapper, "SetAllPoints", (), dbg);
        }
        for anchors in children_named(region, "Anchors") {
            for anchor in children_named(anchors, "Anchor") {
                let Some(point) = anchor.attr("point") else {
                    self.report
                        .warnings
                        .push(format!("{dbg}: region <Anchor> without a point; skipped"));
                    continue;
                };
                let rel_point = anchor.attr("relativePoint").unwrap_or(point).to_string();
                let rel_to: Option<String> = anchor
                    .attr("relativeTo")
                    .map(|r| framexml::resolve_name(r, parent_name));
                let (x, y) = children_named(anchor, "Offset")
                    .next()
                    .map(abs_dim)
                    .unwrap_or((None, None));
                let args = (
                    point.to_string(),
                    rel_to,
                    rel_point,
                    x.unwrap_or(0.0),
                    y.unwrap_or(0.0),
                );
                let d = super::DeferredAnchor {
                    wrapper: wrapper.clone(),
                    region: true,
                    args,
                    dbg: dbg.to_string(),
                };
                // Deferred while the enclosing subtree has not built its target yet.
                self.apply_anchor(d, true);
            }
        }
    }

    pub(super) fn apply_region_visual(
        &mut self,
        region: &Element,
        region_wrapper: &Table,
        is_texture: bool,
        dbg: &str,
    ) {
        let color = children_named(region, "Color").next().map(color_of);
        if is_texture {
            if let Some(file) = region.attr("file") {
                // A `<Color>` beside `file=` is discarded, not a tint: `0x76fe20` applies it in its
                // child loop, then the file, read after (`0x770102`), overwrites the same slot.
                self.call_region(region_wrapper, "SetTexture", file.to_string(), dbg);
            } else if let Some(c) = color {
                // No file: a solid fill, an 8×8 texture in the reference, so its alpha multiplies
                // with `SetVertexColor` rather than being replaced by it.
                self.call_region(region_wrapper, "SetTexture", (c[0], c[1], c[2], c[3]), dbg);
            }
            // `<Gradient>` writes the vertex colours (`+0xb8`, `0x77304d`), which `file=` leaves
            // alone, so unlike `<Color>` it tints the art. MinColor is the first stop and MaxColor
            // the second; the reference's stop-to-vertex order is untraced (`0x7700b0`).
            if let Some(g) = children_named(region, "Gradient").next() {
                let stop = |tag: &str| children_named(g, tag).next().map(color_of);
                // A gradient missing a stop is skipped: an absent stop is not a black one.
                if let (Some(min), Some(max)) = (stop("MinColor"), stop("MaxColor")) {
                    self.call_region(
                        region_wrapper,
                        "SetGradientAlpha",
                        (
                            g.attr("orientation").unwrap_or_default().to_string(),
                            min[0],
                            min[1],
                            min[2],
                            min[3],
                            max[0],
                            max[1],
                            max[2],
                            max[3],
                        ),
                        dbg,
                    );
                }
            }
            if let Some(tc) = tex_coords_of(region) {
                self.call_region(region_wrapper, "SetTexCoord", tc, dbg);
            }
            if let Some(mode) = region.attr("alphaMode") {
                self.call_region(region_wrapper, "SetBlendMode", mode.to_string(), dbg);
            }
        } else {
            if let Some(text) = region.attr("text") {
                // A global-string key, not a literal (`0x703bf0`).
                let text = self.resolve_text(text, dbg);
                self.call_region(region_wrapper, "SetText", Some(text), dbg);
            }
            if let Some(c) = color {
                self.call_region(
                    region_wrapper,
                    "SetVertexColor",
                    (c[0], c[1], c[2], c[3]),
                    dbg,
                );
            }
        }
    }

    /// The font object a FontString's `inherits=` names, directly or through virtual FontString
    /// templates; `None` when the chain ends without one. Bounded, as a registry can hold a cycle.
    pub(super) fn font_object_through_templates(&self, inherits: &str) -> Option<String> {
        const MAX_HOPS: usize = 8;
        let model = self.model();
        let fonts = model.framexml_fonts.borrow();
        let templates = model.framexml_templates.borrow();
        // One name, not a comma list: 1.12's lookup has no splitter.
        for entry in [inherits] {
            let mut name = entry.to_string();
            for _ in 0..MAX_HOPS {
                if fonts.contains_key(&name) || fonts.keys().any(|k| k.eq_ignore_ascii_case(&name))
                {
                    return Some(name);
                }
                let Some(next) = templates
                    .get(&name)
                    .and_then(|t| t.attr("inherits"))
                    .map(str::to_string)
                else {
                    break;
                };
                name = next;
            }
        }
        None
    }

    /// Whether `name` is a registered font object: the reference's flat, case-insensitive lookup
    /// (`0x783870`, `SStrCmpI`), with no template walk.
    pub(super) fn is_font_object(&self, name: &str) -> bool {
        let model = self.model();
        let fonts = model.framexml_fonts.borrow();
        fonts.contains_key(name) || fonts.keys().any(|k| k.eq_ignore_ascii_case(name))
    }

    /// A `<FontString>`'s font: `inherits=` through [`Self::font_object_through_templates`], then
    /// `font=`, a font object's name first and a file only on a miss (`0x770f40`, `0x771104`).
    pub(super) fn apply_fontstring_font(&mut self, region: &Element, wrapper: &Table, dbg: &str) {
        if let Some(name) = region.attr("inherits") {
            // The expanded element keeps the instance's `inherits=`, a template's name, so the
            // chain is walked to its font object. An unresolved name passes through unchanged:
            // `SetFontObject("")` would clear the font object where a bad name warns.
            let resolved = self
                .font_object_through_templates(name)
                .unwrap_or_else(|| name.to_string());
            if let Err(e) = wrapper.call_method::<()>("SetFontObject", resolved) {
                self.warn_once(
                    &format!("fontobj:{name}"),
                    format!("{dbg}: <FontString inherits=\"{name}\">: {e}"),
                );
            }
        }
        // `font=` gates `<FontHeight>` and `outline=` (`0x7710e1`-`0x771254`): a font object's
        // name links it and skips them (`0x771114`, `0x771119`), any other value is a file they
        // qualify (`0x77110b`), and with no `font=` they are never read (`0x7710fa`), so
        // `ZoneText.xml`'s `AutoFollowStatusText` draws at GameFontNormal's 12, not its own 20.
        match region.attr("font") {
            Some(name) if self.is_font_object(name) => {
                if let Err(e) = wrapper.call_method::<()>("SetFontObject", name.to_string()) {
                    self.warn_once(
                        &format!("fontobj:{name}"),
                        format!("{dbg}: <FontString font=\"{name}\">: {e}"),
                    );
                }
            }
            Some(path) => {
                // Applied directly, not through the Lua `SetFont`, which requires a height
                // (`0x87c69c`) where XML may omit it.
                let height = children_named(region, "FontHeight")
                    .next()
                    .and_then(abs_value);
                let outline = region.attr("outline").map(str::to_string);
                if let Err(e) = crate::script::apply_font_parts(
                    self.lua,
                    wrapper,
                    Some(path.to_string()),
                    height,
                    outline,
                ) {
                    self.report
                        .errors
                        .push(format!("{dbg}: region font attrs: {e}"));
                }
            }
            None => {}
        }
    }
}
