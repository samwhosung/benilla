use mlua::Table;

use crate::framexml::{self, Element};
use crate::layout::Len;
use crate::script::binding_abi::s_str_to_int;
use crate::script::object;

use super::{children_named, Loader};

impl Loader<'_> {
    /// The pre-load hook's `id` (`0x769770`), read on the frame's own node alone: a non-empty one
    /// through `SStrToInt`, a negative one skipped (`0x7697ae`-`0x7697b7`).
    pub(super) fn apply_preload_id(&mut self, own: &Element, wrapper: &Table, dbg: &str) {
        if let Some(id) = own.attr("id").filter(|v| !v.is_empty()) {
            let n = s_str_to_int(id);
            if n >= 0 {
                self.call(wrapper, "SetID", n, dbg);
            }
        }
    }

    /// One `<HitRectInsets>` arm of the child loop: the mouse rect, inset from the frame's, when
    /// the inset read succeeds (`0x769cb5`); a side it omits is 0. Each arm applies in turn, so the
    /// last one read wins.
    pub(super) fn apply_hit_rect_insets(&mut self, ins: &Element, wrapper: &Table, dbg: &str) {
        if let Some(v) = self.inset(ins) {
            let [l, r, t, b] = self.script_of(v);
            self.call(wrapper, "SetHitRectInsets", (l, r, t, b), dbg);
            crate::script::xml_layout::hit_rect_insets(self.lua, wrapper, v);
        }
    }

    /// One `<ResizeBounds>` arm (`0x769baa`): its first `<minResize>` and `<maxResize>`, both
    /// pairs always written, an absent one as 0 (`0x769bc8`-`0x769c28`), so a lone `<minResize>`
    /// resets the max to unbounded. A `<Size>` outside them is not clamped.
    pub(super) fn apply_resize_bounds(&mut self, rb: &Element, wrapper: &Table, dbg: &str) {
        for (tag, verb) in [("minResize", "SetMinResize"), ("maxResize", "SetMaxResize")] {
            let v = children_named(rb, tag)
                .next()
                .map_or([Len::Script(0.0); 2], |d| self.dimension(d).v);
            let [x, y] = self.script_of(v);
            self.call(wrapper, verb, (x, y), dbg);
            crate::script::xml_layout::resize_bound(self.lua, wrapper, tag == "maxResize", v);
        }
    }

    /// One level's flag attributes, as `0x769820` reads them on each node after its geometry
    /// (`0x769881`-`0x769b17`), the template first (`0x76985c`): each is skipped when empty (a
    /// first-byte test), so an instance's `""` leaves its template's value.
    pub(super) fn apply_level_flags(&mut self, level: &Element, wrapper: &Table, dbg: &str) {
        // `hidden` hides or shows (`0x7698a2`-`0x7698c4`).
        match level.attr_bool_opt("hidden") {
            Some(true) => self.call(wrapper, "Hide", (), dbg),
            Some(false) => self.call(wrapper, "Show", (), dbg),
            None => {}
        }
        // `toplevel`, `movable` and `resizable` set or clear bits 0x1, 0x100 and 0x200 of the flag
        // word through the setter Lua uses (`0x76a3c0`, at `0x7698ec`, `0x769916`, `0x769940`).
        for (attr, verb) in [
            ("toplevel", "SetToplevel"),
            ("movable", "SetMovable"),
            ("resizable", "SetResizable"),
        ] {
            if let Some(on) = level.attr_bool_opt(attr) {
                self.call(wrapper, verb, on, dbg);
            }
        }
        // An unknown `frameStrata=` is logged at severity 1, the name or `<unnamed>` (`0x878658`,
        // `0x76999c`), and skipped, not raised as the Lua binding would (`0x774360`):
        // `SetFrameStrata` runs only on a hit (`0x769971`), so the earlier level's stands.
        if let Some(strata) = level.attr("frameStrata").filter(|v| !v.is_empty()) {
            if crate::script::object::strata_from_str(strata).is_some() {
                self.call(wrapper, "SetFrameStrata", strata.to_string(), dbg);
            } else {
                let name = self.object_name(wrapper);
                self.status_warning(format!("Frame {name}: Unknown frame strata: {strata}"));
            }
        }
        // `frameLevel=` with `SStrToInt` (`0x7699c5`): above 0 it is set (`0x76a4f0`, the setter
        // `SetFrameLevel` calls), else logged (`0x878628`) and the level kept, so an instance's
        // `"0"` leaves its template's level.
        if let Some(value) = level.attr("frameLevel").filter(|v| !v.is_empty()) {
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
        if let Some(alpha) = level.attr("alpha").filter(|v| !v.is_empty()) {
            let a = crate::script::binding_abi::s_str_to_float(alpha);
            self.call(wrapper, "SetAlpha", a, dbg);
        }
        // `enableMouse` and `enableKeyboard` only ever enable (`0x769aaf`, `0x769adb`), the
        // keyboard both key kinds (`0x769ae8`, `0x769af3`), so `"false"` undoes no template.
        if level.attr_bool("enableMouse") {
            self.call(wrapper, "EnableMouse", true, dbg);
        }
        if level.attr_bool("enableKeyboard") {
            self.call(wrapper, "EnableKeyboard", true, dbg);
        }
        // `clampedToScreen` sets or clears the layout's screen clamp, geometry flag `0x10`
        // (`0x769b17` → `0x768cc0`).
        if let Some(on) = level.attr_bool_opt("clampedToScreen") {
            self.call(wrapper, "SetClampedToScreen", on, dbg);
        }
    }

    /// One `<TitleRegion>` arm of the child loop (`0x769b2f`-`0x769ba5`): a new drag-handle region
    /// for each element, the frame's from then on ([`crate::script::object::xml_title_region`]),
    /// so an instance's element replaces its template's whole and none of the template's anchors
    /// reach the region the frame keeps. Its layout load is `0x767800` itself (`0x81c528[2]`,
    /// called at `0x769b8a`), which never re-enters `inherits`: the node alone.
    pub(super) fn apply_title_region(
        &mut self,
        tr: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        let region = match crate::script::object::xml_title_region(self.lua, wrapper) {
            Ok(r) => r,
            Err(e) => {
                self.report.fault(format!("{dbg}: <TitleRegion>: {e}"));
                return;
            }
        };
        self.apply_geometry(tr, &region, true, self_name, dbg);
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
            let d = super::XmlAnchor {
                wrapper: wrapper.clone(),
                region,
                args,
                offset: children_named(anchor, "Offset").next().cloned(),
                rel_raw: rel_raw.unwrap_or_default().to_string(),
                dbg: dbg.to_string(),
            };
            self.apply_anchor(d);
        }
    }
}
