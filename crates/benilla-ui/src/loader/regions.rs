use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::status;

use super::{children_named, color_of, Loader};

/// Whether a region reads its own font attributes, the reference's `FONTSTRING+0x12c`: set by the
/// ctor (`0x770de7`) and cleared only on a Button's `<NormalText>` (`0x778b7b`), whose string then
/// skips them (`0x7710db` → `0x771468`) for the button's Normal font to take (`0x783c30`): an
/// `inherits` font object, `font=`, `spacing`, the justifies, `<Color>` and `<Shadow>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FontAttrs {
    /// Every other region, a Button's `<ButtonText>` included.
    Own,
    /// A Button's `<NormalText>`: its font attributes are the button's Normal font's.
    Disowned,
}

impl Loader<'_> {
    /// One `<Layers>` (`0x769d70`): each `<Texture>` and `<FontString>`, created on its layer.
    pub(super) fn apply_layers(
        &mut self,
        layers: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        for layer in children_named(layers, "Layer") {
            let level = layer.attr("level").unwrap_or("ARTWORK").to_string();
            for region in &layer.children {
                let is_texture = region.tag.eq_ignore_ascii_case("Texture");
                let is_fontstring = region.tag.eq_ignore_ascii_case("FontString");
                if !is_texture && !is_fontstring {
                    self.report.warnings.push(format!(
                        "{dbg}: <Layer> child <{}> is not a Texture/FontString; skipped",
                        region.tag
                    ));
                    continue;
                }
                self.build_region(region, wrapper, parent_name, &level, is_texture, dbg);
            }
        }
    }

    /// The texture a widget element's child (`<BarTexture>`, `<ThumbTexture>`, a ColorSelect
    /// texture) names, built as the shared texture adder does (`0x6f26f0`): the same region a
    /// `<Layers>` texture is, named, published, loaded level by level and given its implicit
    /// all-points. The widget then installs the object it returns.
    pub(super) fn build_texture_child(
        &mut self,
        raw: &Element,
        wrapper: &Table,
        parent_name: &str,
        level: &str,
        dbg: &str,
    ) -> Option<Table> {
        self.build_region(raw, wrapper, parent_name, level, true, dbg)
    }

    /// One `<Layer>` region on `wrapper`, created on `level` and loaded: its name (`$parent`
    /// expanded, published as a global, read from `raw` alone, `0x6f2751`), then its `LoadXML`
    /// level by level ([`Loader::levels`], a FontString's up to a font,
    /// [`Loader::fontstring_levels`]; `inherits=` may name a region template, in the frames' one
    /// registry, `0x6ee500`), then the implicit creation anchor, run after the region's `LoadXML`
    /// as the reference does (`0x7701c0`, `0x771480`); an anchored region is left alone. `None`
    /// when the creation raised.
    fn build_region(
        &mut self,
        raw: &Element,
        wrapper: &Table,
        parent_name: &str,
        level: &str,
        is_texture: bool,
        dbg: &str,
    ) -> Option<Table> {
        let levels = if is_texture {
            self.levels(raw)
        } else {
            self.fontstring_levels(raw)
        };
        let rname: Option<String> = raw
            .name()
            .map(|raw| framexml::resolve_name(raw, parent_name));
        let method = if is_texture {
            "CreateTexture"
        } else {
            "CreateFontString"
        };
        let region_wrapper: Table =
            match wrapper.call_method(method, (rname, Some(level.to_string()))) {
                Ok(w) => w,
                Err(e) => {
                    self.report.fault(format!("{dbg}: {method}: {e}"));
                    return None;
                }
            };
        for node in &levels {
            if is_texture {
                self.load_texture_level(node, &region_wrapper, parent_name, dbg);
            } else {
                let attrs = FontAttrs::Own;
                self.load_fontstring_level(node, &region_wrapper, parent_name, attrs, true, dbg);
            }
        }
        if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region_wrapper) {
            self.report.fault(format!("{dbg}: implicit anchor: {e}"));
        }
        Some(region_wrapper)
    }

    /// A frame's direct-child `<FontString>`, on the OVERLAY layer; a text frame's loads into its
    /// own font instead ([`Loader::apply_text_font_children`]).
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
            let levels = self.fontstring_levels(region);
            let rname: Option<String> = region
                .name()
                .map(|raw| framexml::resolve_name(raw, parent_name));
            let region_wrapper: Table = match wrapper
                .call_method("CreateFontString", (rname, Some("OVERLAY".to_string())))
            {
                Ok(w) => w,
                Err(e) => {
                    self.report
                        .fault(format!("{dbg}: CreateFontString (special): {e}"));
                    continue;
                }
            };
            for node in &levels {
                let attrs = FontAttrs::Own;
                self.load_fontstring_level(node, &region_wrapper, parent_name, attrs, true, dbg);
            }
            // The implicit creation anchor, as in `apply_layers`.
            if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region_wrapper) {
                self.report.fault(format!("{dbg}: implicit anchor: {e}"));
            }
        }
    }

    /// `"<kind> <name>: Missing font height in <tag> element"` at severity 1 into the document's
    /// record: `0x878e10` for a FontString (`0x77117d`), `0x87a4f0` for a Font or a SimpleHTML
    /// element font (`0x783d9a`), with the object's name or `<unnamed>` (`0x84c7f0`) and the node's tag.
    pub(super) fn report_missing_font_height(&mut self, kind: &str, name: &str, tag: &str) {
        let line = format!("{kind} {name}: Missing font height in {tag} element");
        self.report.status.report(status::WARNING, line.clone());
        self.report.warnings.push(line);
    }

    /// A FontString's `nonspacewrap` and `maxLines` (`0x77103f`-`0x7710cd`), which the loader reads
    /// ahead of the font attributes, so a Button's `<NormalText>` takes them too. A boolean goes
    /// through the string-to-bool `0x6f1b30`; `maxLines` through `0x64ade0`, its leading digits
    /// (none is 0), wrapping in 32 bits.
    pub(super) fn apply_wrap_attrs(&mut self, region: &Element, wrapper: &Table, dbg: &str) {
        let non_space_wrap = region.attr_bool_opt("nonspacewrap");
        let max_lines = region
            .attr("maxLines")
            .filter(|v| !v.is_empty())
            .map(leading_digits);
        if non_space_wrap.is_none() && max_lines.is_none() {
            return;
        }
        if let Err(e) =
            crate::script::apply_wrap_attrs(self.lua, wrapper, non_space_wrap, max_lines)
        {
            self.report.fault(format!("{dbg}: region wrap attrs: {e}"));
        }
    }

    /// One level of a `<FontString>`'s `LoadXML` (`0x770f40`), after its `inherits` level: an
    /// `inherits` font object, the geometry, `hidden`, `text`, the wrap attributes (no `alpha`,
    /// which only the frame loader reads, `0x769a10`), then, when the
    /// string owns its font attributes ([`FontAttrs`], `0x7710db`), `font=`, `spacing`,
    /// `justifyV`, `justifyH` and the `<Color>` and `<Shadow>` children, in that order. A `font=`
    /// file needs this node's own first `<FontHeight>` above 0 (`0x77112f`-`0x771154`); without
    /// one the level reports `0x878e10` and returns (`0x771191`), so nothing after the font is
    /// read on it, whatever height a template gave. A file that does not load only skips the
    /// font. `with_text` is false for a Button's label, whose text the button sets.
    pub(super) fn load_fontstring_level(
        &mut self,
        node: &Element,
        wrapper: &Table,
        parent_name: &str,
        attrs: FontAttrs,
        with_text: bool,
        dbg: &str,
    ) {
        let own = attrs == FontAttrs::Own;
        // `inherits=` names a font object first (`0x770f69`), linked only when the string owns
        // its font attributes (`0x770f72`), else a template, already loaded as a level of its own
        // ([`Loader::fontstring_levels`]). A name that is neither is logged (`0x8786a8`,
        // `0x770fa3`), whoever owns the font.
        if let Some(name) = node.attr("inherits").filter(|n| !n.is_empty()) {
            if self.is_font_object(name) {
                if own {
                    self.set_font_object(wrapper, "inherits", name, dbg);
                }
            } else if !self.model().framexml_templates.borrow().contains(name) {
                self.status_warning(format!("Couldn't find inherited node: {name}"));
            }
        }
        self.apply_geometry(node, wrapper, true, parent_name, dbg);
        match node.attr_bool_opt("hidden") {
            Some(true) => self.call_region(wrapper, "Hide", (), dbg),
            Some(false) => self.call_region(wrapper, "Show", (), dbg),
            None => {}
        }
        // An empty `text` is skipped (`0x77101b`), so a template's text stands. The string's own
        // `SetText` (`0x77103a` → `0x771d80`), not Lua's, which raises on a string with no font
        // (`0x79d7e5`); `LoadXML` sets the text before the font.
        if let Some(raw) = node.attr("text").filter(|t| with_text && !t.is_empty()) {
            let text = self.resolve_text(raw, dbg);
            if let Err(e) = crate::script::region::store_text_of(self.lua, wrapper, Some(text)) {
                self.report.fault(format!("{dbg}: region SetText: {e}"));
            }
        }
        self.apply_wrap_attrs(node, wrapper, dbg);
        if !own {
            return;
        }
        // `font=`, empty as absent (`0x7710f7`): a font object's name links it (`0x771114`), any
        // other value is a file its own `<FontHeight>` and `outline=` qualify (`0x77110b`), so
        // `ZoneText.xml`'s `AutoFollowStatusText` draws at GameFontNormal's 12, not its own 20.
        match node.attr("font").filter(|f| !f.is_empty()) {
            Some(name) if self.is_font_object(name) => {
                self.set_font_object(wrapper, "font", name, dbg);
            }
            Some(path) => {
                // The first `<FontHeight>`, read whatever it answers (`0x771141`), above 0.
                let height = children_named(node, "FontHeight")
                    .next()
                    .map(|h| self.value(h))
                    .filter(|h| *h > 0.0);
                let Some(height) = height else {
                    let name = self.object_name(wrapper);
                    self.report_missing_font_height("FontString", &name, &node.tag);
                    return;
                };
                // The flags word, set whole on this leg (`0x771194`-`0x771214`): `outline=` is
                // NORMAL or THICK, and any non-empty `monochrome=` adds the flag, even "false".
                let flags = (
                    node.attr("outline")
                        .map(crate::script::Outline::parse)
                        .unwrap_or_default(),
                    node.attr("monochrome").is_some_and(|v| !v.is_empty()),
                );
                // A file that fails to load is logged (`0x878dc8`, `0x771226`-`0x771251`) and the
                // level goes on to `spacing`, the string keeping the font it had.
                match crate::script::load_xml_font_file(self.lua, wrapper, path, height, flags) {
                    Ok(true) => {}
                    Ok(false) => {
                        let name = self.object_name(wrapper);
                        self.status_warning(format!(
                            "FontString {name}: Unable to load font file {path}"
                        ));
                    }
                    Err(e) => self.report.fault(format!("{dbg}: region font attrs: {e}")),
                }
            }
            None => {}
        }
        // `spacing=` (`0x771257`, `SStrToFloat` at `0x77126d`) goes through the setter and clears inherit bit `0x10`
        // (`0x771298`), so it outlasts the font object's.
        if let Some(sp) = node.attr("spacing").filter(|v| !v.is_empty()) {
            self.call_region(wrapper, "SetSpacing", super::xml_float(sp), dbg);
        }
        // A justify token is matched first (`0x6f1990`) and one that matches nothing is skipped
        // (`0x7712c3`, `0x771327`). A match writes its axis's bits of the justify word inline, none
        // for the other axis's token, which zeroes the axis (V `0x7712e1`-`0x7712e7`, H
        // `0x771345`-`0x77134b`), and clears the bits the Lua setter clears, so the setter stands in.
        for (attr, setter) in [("justifyV", "SetJustifyV"), ("justifyH", "SetJustifyH")] {
            if let Some(j) = node
                .attr(attr)
                .filter(|j| crate::justify::parse_bits(j).is_some())
            {
                self.call_region(wrapper, setter, j.to_string(), dbg);
            }
        }
        // Each `<Color>` is the text colour (`0x77f750`) and each `<Shadow>` the shadow
        // (`0x7722a0`), both local to the string, so they outlast the font object's.
        for child in &node.children {
            if child.tag.eq_ignore_ascii_case("Color") {
                let c = color_of(child).fractions();
                self.call_region(wrapper, "SetVertexColor", c, dbg);
            } else if child.tag.eq_ignore_ascii_case("Shadow") {
                let sh = self.shadow_of(child);
                self.call_region(wrapper, "SetShadowColor", sh.color.fractions(), dbg);
                self.call_region(
                    wrapper,
                    "SetShadowOffset",
                    (sh.offset[0], sh.offset[1]),
                    dbg,
                );
            }
        }
    }

    /// `SetFontObject` for an XML `<attr>=` naming `name`, a failure reported once.
    fn set_font_object(&mut self, wrapper: &Table, attr: &str, name: &str, dbg: &str) {
        if let Err(e) = wrapper.call_method::<()>("SetFontObject", name.to_string()) {
            self.warn_once(
                &format!("fontobj:{name}"),
                format!("{dbg}: <FontString {attr}=\"{name}\">: {e}"),
            );
        }
    }

    /// One level of a `<Texture>`'s `LoadXML` (`0x76fe20`), after its `inherits` level: the
    /// geometry, `hidden`, each child in order, then `file=` and `alphaMode`, and no `alpha`, which
    /// only the frame loader reads (`0x769a10`). A `<TexCoords>` sets
    /// all eight coordinates from its own edges over 0, 1, 0, 1, so the last one applied wins whole
    /// (`0x76fef5`, `0x76ffe8`), and any edge outside [0, 1] sets the level's wrap flag, which the
    /// file load takes (`0x770071`, `0x77012d`); a later `SetTexCoord` leaves it, and `SetTexture`
    /// loads with it off. A `<Color>` is a solid fill (`0x770360`), an 8×8 texture in the
    /// reference, so its alpha multiplies with `SetVertexColor` rather than being replaced by it;
    /// the level's `file=`, read after its children (`0x770102`), overwrites the same slot.
    fn load_texture_level(
        &mut self,
        node: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        self.apply_geometry(node, wrapper, true, parent_name, dbg);
        match node.attr_bool_opt("hidden") {
            Some(true) => self.call_region(wrapper, "Hide", (), dbg),
            Some(false) => self.call_region(wrapper, "Show", (), dbg),
            None => {}
        }
        let mut wrap = false;
        for child in &node.children {
            if child.tag.eq_ignore_ascii_case("TexCoords") {
                let (l, r, t, b) = super::tex_coords(child);
                // Not `contains`: a NaN edge is in range here.
                #[allow(clippy::manual_range_contains)]
                let out_of_range = |edge: f32| edge < 0.0 || edge > 1.0;
                wrap |= [l, r, t, b].into_iter().any(out_of_range);
                self.call_region(wrapper, "SetTexCoord", (l, r, t, b), dbg);
            } else if child.tag.eq_ignore_ascii_case("Color") {
                self.call_region(wrapper, "SetTexture", color_of(child).fractions(), dbg);
            } else if child.tag.eq_ignore_ascii_case("Gradient") {
                self.apply_gradient(child, wrapper, dbg);
            }
        }
        // A file that fails to load is logged (`0x878d24`) and the texture filled opaque green
        // (`0xff00ff00` through `0x770360`, `0x77016a`), the reference's tell for a bad path.
        if let Some(file) = node.attr("file").filter(|f| !f.is_empty()) {
            match crate::script::load_texture_file(self.lua, wrapper, file, wrap) {
                Ok(crate::script::FileLoad::Failed) => {
                    let name = self.object_name(wrapper);
                    self.status_warning(format!(
                        "Texture {name}: Unable to load texture file {file}"
                    ));
                    self.call_region(wrapper, "SetTexture", (0.0, 1.0, 0.0, 1.0), dbg);
                }
                Ok(_) => {}
                Err(e) => self.report.fault(format!("{dbg}: region SetTexture: {e}")),
            }
        }
        if let Some(mode) = node.attr("alphaMode") {
            if let Err(e) = crate::script::load_blend_mode(self.lua, wrapper, mode) {
                self.report.fault(format!("{dbg}: region alphaMode: {e}"));
            }
        }
    }

    /// A texture's `<Gradient>`: it writes the vertex colours (`+0xb8`, `0x7700f2` → `0x77f910`),
    /// which `file=` leaves alone, so unlike `<Color>` it tints the art. `0x6f2630` stages both
    /// stops as 0 (`0x7700d8`), so a missing one is transparent black, and reads a missing
    /// orientation as HORIZONTAL; an unknown one stays HORIZONTAL and is reported.
    fn apply_gradient(&mut self, g: &Element, wrapper: &Table, dbg: &str) {
        let stop = |tag: &str| {
            children_named(g, tag)
                .next()
                .map_or(crate::color::Rgba8::default(), color_of)
        };
        let (min, max) = (stop("MinColor").fractions(), stop("MaxColor").fractions());
        let token = g.attr("orientation").unwrap_or_default();
        let vertical = match crate::script::region::orientation(token) {
            Some(v) => v,
            None if token.is_empty() => false,
            None => {
                let line = format!("Unknown orientation {token} in element {}", g.tag);
                self.status_warning(line);
                false
            }
        };
        self.call_region(
            wrapper,
            "SetGradientAlpha",
            (
                if vertical { "VERTICAL" } else { "HORIZONTAL" },
                min.0,
                min.1,
                min.2,
                min.3,
                max.0,
                max.1,
                max.2,
                max.3,
            ),
            dbg,
        );
    }

    /// The font object a FontString's `inherits=` names, directly or through virtual FontString
    /// templates; `None` when the chain ends without one. Bounded, as a registry can hold a cycle.
    pub(super) fn font_object_through_templates(&self, inherits: &str) -> Option<String> {
        const MAX_HOPS: usize = 8;
        let model = self.model();
        let templates = model.framexml_templates.borrow();
        // One name, not a comma list: 1.12's lookup has no splitter.
        for entry in [inherits] {
            let mut name = entry.to_string();
            for _ in 0..MAX_HOPS {
                if model.fonts.id(&name).is_some() {
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

    /// Whether `name` is a named font object, declared by `<Font>` or made by `CreateFont`: the
    /// reference's one font table (`0x783870`, which `CreateFont` fills too, `0x706077`), looked
    /// up flat and ignoring case, with no template walk.
    pub(super) fn is_font_object(&self, name: &str) -> bool {
        self.model().fonts.id(name).is_some()
    }
}

/// The client's decimal parse `0x64ade0`: the leading digits of `s`, wrapping in 32 bits, and 0
/// when `s` does not open with one.
fn leading_digits(s: &str) -> u32 {
    s.bytes().take_while(u8::is_ascii_digit).fold(0u32, |n, b| {
        n.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
    })
}
