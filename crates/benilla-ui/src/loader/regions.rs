use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::status;

use super::{abs_value, children_named, color_of, Loader};

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
    /// level by level ([`Loader::levels`]; `inherits=` may name a region template, in the frames'
    /// one registry, `0x6ee500`), then the implicit creation anchor, run after the region's
    /// LoadXML as the reference does (`0x7701c0`, `0x771480`); an anchored region is left alone.
    /// `None` when the creation raised.
    fn build_region(
        &mut self,
        raw: &Element,
        wrapper: &Table,
        parent_name: &str,
        level: &str,
        is_texture: bool,
        dbg: &str,
    ) -> Option<Table> {
        let levels = self.levels(raw);
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
            let levels = self.levels(region);
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
                            .fault(format!("{dbg}: CreateFontString (special): {e}"));
                        continue;
                    }
                },
            };
            for node in &levels {
                let attrs = FontAttrs::Own;
                self.load_fontstring_level(node, &region_wrapper, parent_name, attrs, true, dbg);
            }
            // The implicit creation anchor, as in `apply_layers`; an EditBox's adopt replaces it.
            if let Err(e) = crate::script::implicit_creation_anchor_lua(self.lua, &region_wrapper) {
                self.report.fault(format!("{dbg}: implicit anchor: {e}"));
            }
            // It is the EditBox's text region by slot, never found by a search, which could take a
            // `<Layers>` string; the box's insets anchor it.
            if el.tag.eq_ignore_ascii_case("EditBox") {
                if let Err(e) =
                    crate::script::adopt_text_region(self.lua(), wrapper, &region_wrapper)
                {
                    self.report
                        .fault(format!("{dbg}: adopt special FontString: {e}"));
                }
                if let Some(bytes) = levels.iter().rev().find_map(|l| l.attr("bytes")) {
                    if let Err(e) = crate::script::apply_xml_bytes(self.lua(), wrapper, bytes) {
                        self.report.fault(format!("{dbg}: FontString bytes: {e}"));
                    }
                }
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
    /// `inherits` font object, the geometry, `hidden`, `text`, the wrap attributes, then, when the
    /// string owns its font attributes ([`FontAttrs`], `0x7710db`), `font=`, `spacing`,
    /// `justifyV`, `justifyH` and the `<Color>` and `<Shadow>` children, in that order. A `font=`
    /// file needs this node's own first `<FontHeight>` above 0 (`0x77112f`-`0x771154`); without
    /// one the level reports `0x878e10` and returns (`0x771191`), so nothing after the font is
    /// read on it, whatever height a template gave. `with_text` is false for a Button's label,
    /// whose text the button sets.
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
        // its font attributes (`0x770f72`), else a template, loaded as a level of its own. Any
        // other name passes through and is reported: `SetFontObject("")` would clear the font
        // object where a bad name warns.
        if let Some(name) = node.attr("inherits").filter(|n| !n.is_empty()) {
            let template = self.model().framexml_templates.borrow().contains(name);
            if own && (self.is_font_object(name) || !template) {
                self.set_font_object(wrapper, "inherits", name, dbg);
            }
        }
        self.apply_geometry(node, wrapper, true, parent_name, dbg);
        match node.attr_bool_opt("hidden") {
            Some(true) => self.call_region(wrapper, "Hide", (), dbg),
            Some(false) => self.call_region(wrapper, "Show", (), dbg),
            None => {}
        }
        if let Some(a) = node
            .attr("alpha")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            self.call_region(wrapper, "SetAlpha", a, dbg);
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
                let height = children_named(node, "FontHeight")
                    .next()
                    .and_then(abs_value)
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
                if let Err(e) = crate::script::apply_font_parts(
                    self.lua,
                    wrapper,
                    Some(path.to_string()),
                    Some(height),
                    Some(flags),
                ) {
                    self.report.fault(format!("{dbg}: region font attrs: {e}"));
                }
            }
            None => {}
        }
        // `spacing=` (`0x771257`) goes through the setter and clears inherit bit `0x10`
        // (`0x771298`), so it outlasts the font object's.
        if let Some(sp) = node.attr("spacing").filter(|v| !v.is_empty()) {
            self.call_region(wrapper, "SetSpacing", crate::framexml::atof(sp), dbg);
        }
        if let Some(j) = node.attr("justifyV").filter(|v| !v.is_empty()) {
            self.call_region(wrapper, "SetJustifyV", j.to_string(), dbg);
        }
        if let Some(j) = node.attr("justifyH").filter(|v| !v.is_empty()) {
            self.call_region(wrapper, "SetJustifyH", j.to_string(), dbg);
        }
        // Each `<Color>` is the text colour (`0x77f750`) and each `<Shadow>` the shadow
        // (`0x7722a0`), both local to the string, so they outlast the font object's.
        for child in &node.children {
            if child.tag.eq_ignore_ascii_case("Color") {
                let c = color_of(child);
                self.call_region(wrapper, "SetVertexColor", (c[0], c[1], c[2], c[3]), dbg);
            } else if child.tag.eq_ignore_ascii_case("Shadow") {
                let sh = super::shadow_of(child);
                let c = sh.color;
                self.call_region(wrapper, "SetShadowColor", (c[0], c[1], c[2], c[3]), dbg);
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
    /// geometry, `hidden`, each child in order, then `file=` and `alphaMode`. A `<TexCoords>` sets
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
        if let Some(a) = node
            .attr("alpha")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            self.call_region(wrapper, "SetAlpha", a, dbg);
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
                let c = color_of(child);
                self.call_region(wrapper, "SetTexture", (c[0], c[1], c[2], c[3]), dbg);
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
        let stop = |tag: &str| children_named(g, tag).next().map_or([0.0; 4], color_of);
        let (min, max) = (stop("MinColor"), stop("MaxColor"));
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
}

/// The client's decimal parse `0x64ade0`: the leading digits of `s`, wrapping in 32 bits, and 0
/// when `s` does not open with one.
fn leading_digits(s: &str) -> u32 {
    s.bytes().take_while(u8::is_ascii_digit).fold(0u32, |n, b| {
        n.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
    })
}
