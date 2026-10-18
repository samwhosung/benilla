use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::layout::Len;
use crate::script::binding_abi::{s_str_to_float, s_str_to_int};
use crate::script::object;

use super::{children_named, color_of, Levels, Loader};

impl Loader<'_> {
    /// A frame element's LoadXML attributes and simple children (`0x769820`), and a model pane's
    /// own (`0x76cac0`): from the merged `el`, and `frameLevel` from each of `levels`.
    pub(super) fn apply_attrs(
        &mut self,
        el: &Element,
        levels: &Levels,
        wrapper: &Table,
        dbg: &str,
    ) {
        if el.attr_bool("hidden") {
            self.call(wrapper, "Hide", (), dbg);
        }
        // An unknown `frameStrata=` is logged (`0x7699a4`) and skipped, not raised as the Lua
        // binding would (`0x774360`): `SetFrameStrata` runs only on a hit (`0x769971`), so the
        // frame keeps the strata it had.
        if let Some(strata) = el.attr("frameStrata") {
            if crate::script::object::strata_from_str(strata).is_some() {
                self.call(wrapper, "SetFrameStrata", strata.to_string(), dbg);
            } else {
                self.report
                    .warnings
                    .push(format!("{dbg}: Unknown frame strata: {strata}"));
            }
        }
        // `frameLevel=` per level, read with `SStrToInt` (`0x7699c5`): above 0 it is set (`0x76a4f0`,
        // the setter `SetFrameLevel` calls), else logged (`0x878628`) and the level kept, so an
        // instance's `"0"` leaves its template's level.
        for level in levels {
            let Some(value) = level.attr("frameLevel").filter(|v| !v.is_empty()) else {
                continue;
            };
            let n = s_str_to_int(value);
            if n > 0 {
                self.call(wrapper, "SetFrameLevel", n, dbg);
            } else {
                let name = self.object_name(wrapper);
                self.status_warning(format!("Frame {name}: Unknown frame level: {value}"));
            }
        }
        // `0x769a10`: a non-empty `alpha` through `SStrToFloat` (`0x64aaa0`), clamped with NaN
        // at 1 and handed to the alpha walk as Lua's `SetAlpha` hands it (`0x769a76`-`0x769a8a`).
        if let Some(alpha) = el.attr("alpha").filter(|v| !v.is_empty()) {
            let a = crate::script::binding_abi::s_str_to_float(alpha);
            self.call(wrapper, "SetAlpha", a, dbg);
        }

        // A model pane's `file=` is `SetModel` (`0x76cac0`).
        let model_kind = super::model_kind_tag(&el.tag);
        if model_kind {
            if let Some(file) = el.attr("file") {
                let text = self.resolve_text(file, dbg);
                self.call(wrapper, "SetModel", text, dbg);
            }
        }
        // A model pane's non-empty `scale=` through `SStrToFloat` is the model's scale, the field
        // `SetModelScale` writes (`0x76cb61`); a value of 0 or less is logged (`0x8788d8`), never
        // clamped (`0x76cb92`). A plain frame's `scale=` is ignored; whether `0x769820` reads one
        // is untraced.
        if let Some(scale) = el.attr("scale").filter(|v| !v.is_empty()) {
            if model_kind {
                let s = s_str_to_float(scale) as f32;
                if s > 0.0 {
                    self.call(wrapper, "SetModelScale", s, dbg);
                } else {
                    let name = self.object_name(wrapper);
                    self.status_warning(format!("Frame {name}: Invalid model scale: {scale}"));
                }
            }
        }
        // A model pane's fog: a non-empty `fogNear`/`fogFar` through `SStrToFloat`, clamped at 0
        // here only (`0x76cbbb`, `0x76cbf3`), where the Lua setters store raw; each `<FogColor>`
        // in the child loop (`0x76cc20`-`0x76cc5d`), the last winning, is read as a `<Color>`
        // (`0x6f23d0`, `0x76cc43`) and set with `SetFogColor`, fog bit included.
        if model_kind {
            for (attr, verb) in [("fogNear", "SetFogNear"), ("fogFar", "SetFogFar")] {
                if let Some(v) = el.attr(attr).filter(|v| !v.is_empty()) {
                    let v = s_str_to_float(v) as f32;
                    self.call(wrapper, verb, v.max(0.0), dbg);
                }
            }
            for fc in children_named(el, "FogColor") {
                self.call(wrapper, "SetFogColor", color_of(fc).fractions(), dbg);
            }
        }
        // A non-empty `id` through `SStrToInt`, a negative one skipped (`0x7697ae`-`0x7697b7`).
        if let Some(id) = el.attr("id").filter(|v| !v.is_empty()) {
            let n = s_str_to_int(id);
            if n >= 0 {
                self.call(wrapper, "SetID", n, dbg);
            }
        }
        // `clampedToScreen`: the layout's screen clamp, geometry flag `0x10` (`0x768cc0`).
        if el.attr_bool("clampedToScreen") {
            self.call(wrapper, "SetClampedToScreen", true, dbg);
        }
        if el.attr_bool("enableMouse") {
            self.call(wrapper, "EnableMouse", true, dbg);
        }
        // `<HitRectInsets>` and `<ResizeBounds>` are arms of the frame's child loop
        // (`0x769b27`-`0x769d22`), so each one applies in turn and the last wins; the merged
        // element holds a template's children first, so an instance's own one wins over it.
        // `<HitRectInsets>`: the mouse rect, inset from the frame's, when the inset read succeeds
        // (`0x769cb5`); a side it omits is 0.
        for ins in children_named(el, "HitRectInsets") {
            if let Some(v) = self.inset(ins) {
                let [l, r, t, b] = self.script_of(v);
                self.call(wrapper, "SetHitRectInsets", (l, r, t, b), dbg);
                crate::script::xml_layout::hit_rect_insets(self.lua, wrapper, v);
            }
        }
        // `<ResizeBounds>` (`0x769baa`): its first `<minResize>` and `<maxResize>`, both pairs
        // always written, an absent one as 0, so a lone `<minResize>` resets the max to unbounded.
        // A `<Size>` outside them is not clamped.
        for rb in children_named(el, "ResizeBounds") {
            for (tag, verb) in [("minResize", "SetMinResize"), ("maxResize", "SetMaxResize")] {
                let v = children_named(rb, tag)
                    .next()
                    .map_or([Len::Script(0.0); 2], |d| self.dimension(d).v);
                let [x, y] = self.script_of(v);
                self.call(wrapper, verb, (x, y), dbg);
                crate::script::xml_layout::resize_bound(self.lua, wrapper, tag == "maxResize", v);
            }
        }
        // `movable`/`resizable`: flag bits 0x100/0x200, via the setter Lua uses (`0x76a3c0`).
        if el.attr_bool("movable") {
            self.call(wrapper, "SetMovable", true, dbg);
        }
        if el.attr_bool("resizable") {
            self.call(wrapper, "SetResizable", true, dbg);
        }
        // `toplevel`: bit 0x1 of the same flag word (`0x7698ec`).
        if el.attr_bool("toplevel") {
            self.call(wrapper, "SetToplevel", true, dbg);
        }
        // `enableKeyboard` enables both key kinds (`0x769ae8`, `0x769af3`).
        if el.attr_bool("enableKeyboard") {
            self.call(wrapper, "EnableKeyboard", true, dbg);
        }
    }

    /// `<TitleRegion>`: the drag handle, built through the idempotent `CreateTitleRegion` so the
    /// element and a later Lua call name one object (`0x769b2a` takes `0x773910`'s path).
    pub(super) fn apply_title_region(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        // Each `<TitleRegion>` in the child loop (`0x769b2f`-`0x769ba5`) loads into the one region.
        for tr in children_named(el, "TitleRegion") {
            let region: Table = match wrapper.call_method("CreateTitleRegion", ()) {
                Ok(r) => r,
                Err(e) => {
                    self.report.fault(format!("{dbg}: CreateTitleRegion: {e}"));
                    return;
                }
            };
            // The title region's layout load is `0x767800` itself (`0x81c528[2]`, called at
            // `0x769b8a`), which never re-enters `inherits`: the node alone.
            self.apply_geometry(tr, &region, true, self_name, dbg);
        }
    }

    /// One node's geometry, the CLayoutFrame part of every frame and region `LoadXML` (`0x767800`),
    /// run once per level ([`Loader::levels`]): the node's first `<Size>`, then its first
    /// `<Anchors>`, whose points are added, or, with none, `setAllPoints="true"` (compared whole,
    /// `0x767879`) as `SetAllPoints`, which clears the points first (`0x767db0`). `setAllPoints`
    /// beside the node's own `<Anchors>` is logged (`0x8784ac`) and ignored, so a template's
    /// all-points and an instance's anchor both stand (`ActionButtonTemplate.xml`'s
    /// `$parentCooldown` over `CooldownFrameTemplate`). `relativePoint` defaults to `point`, and
    /// `relativeTo` expands `$parent` to `parent_name`, the nearest named ancestor's, else `Top`
    /// (`0x76c5b0`).
    pub(super) fn apply_geometry(
        &mut self,
        node: &Element,
        wrapper: &Table,
        region: bool,
        parent_name: &str,
        dbg: &str,
    ) {
        // A `<Size>` the dimension read accepts sets both axes, a missing one as 0
        // (`0x767835`-`0x76784a`); one it refuses sets nothing.
        if let Some(size) = children_named(node, "Size").next() {
            let d = self.dimension(size);
            if d.ok {
                let [w, h] = d.script(self.model().layout_units());
                self.call_on(wrapper, region, "SetWidth", w, dbg);
                self.call_on(wrapper, region, "SetHeight", h, dbg);
                crate::script::xml_layout::size(self.lua, wrapper, region, d.v);
            }
        }
        let set_all_points = node.attr_is_true("setAllPoints");
        let Some(anchors) = children_named(node, "Anchors").next() else {
            if set_all_points {
                self.call_on(wrapper, region, "SetAllPoints", (), dbg);
            }
            return;
        };
        if set_all_points {
            self.status_warning("SETALLPOINTS set to true in frame with anchors (ignored)".into());
        }
        // Every child of the first `<Anchors>`, whatever its tag (`[node+4]`, then `[child+0x1c]`,
        // `0x7679e3`): its `point` through `0x6f1840`, a whole-string match ignoring case; a missing
        // or unknown one is logged (`0x878470`, `0x7679cf`) and the child skipped, a missing one
        // printing `(null)`: the status's `0x41a000` formats through `0x64a8b0` and `0x64a820`
        // to the CRT, whose `__output` (`0x742ba5`) puts `(null)` (`0x871708`) for a null `%s`
        // (`0x742ffc`).
        for anchor in &anchors.children {
            let point = anchor.attr("point");
            let Some(point) = point.filter(|p| object::point_from_str(p).is_some()) else {
                let shown = point.unwrap_or("(null)");
                self.status_warning(format!("Invalid anchor point in frame: {shown}"));
                continue;
            };
            // A non-empty `relativePoint` must parse too (`0x76792d`); an empty or absent one is
            // `point` (`0x767940`).
            let rel_point = match anchor.attr("relativePoint").filter(|r| !r.is_empty()) {
                Some(r) if object::point_from_str(r).is_none() => {
                    self.status_warning(format!("Invalid anchor point in frame: {r}"));
                    continue;
                }
                Some(r) => r,
                None => point,
            };
            // An empty `relativeTo` is the layout parent, as an absent one (`0x76795c`).
            let rel_raw = anchor.attr("relativeTo").filter(|r| !r.is_empty());
            let rel_to = rel_raw.map(|r| framexml::resolve_name(r, parent_name));
            let args = (point.to_string(), rel_to, rel_point.to_string());
            let d = super::DeferredAnchor {
                wrapper: wrapper.clone(),
                region,
                args,
                offset: children_named(anchor, "Offset").next().cloned(),
                rel_raw: rel_raw.unwrap_or_default().to_string(),
                dbg: dbg.to_string(),
            };
            // Deferred while the enclosing subtree has not built its target yet.
            self.apply_anchor(d, true);
        }
    }
}
