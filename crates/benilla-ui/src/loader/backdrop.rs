use mlua::Table;

use crate::framexml::Element;

use super::{abs_value, children_named, color_of, Loader};

impl Loader<'_> {
    /// `<Backdrop>` (LoadXML `0x77e6c0`): the table Lua `SetBackdrop` takes, then the two colours.
    /// A present `<Color>` defaults a missing channel to black, alpha 1 (`0x6f23d0`), not the
    /// white a table `SetBackdrop` leaves.
    pub(super) fn apply_backdrop(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        let Some(bd) = children_named(el, "Backdrop").next() else {
            return;
        };
        let table = match self.build_backdrop_table(bd) {
            Ok(t) => t,
            Err(e) => {
                self.report.errors.push(format!("{dbg}: <Backdrop>: {e}"));
                return;
            }
        };
        self.call(wrapper, "SetBackdrop", table, dbg);
        if let Some(c) = children_named(bd, "Color").next() {
            let [r, g, b, a] = color_of(c);
            self.call(wrapper, "SetBackdropColor", (r, g, b, a), dbg);
        }
        if let Some(c) = children_named(bd, "BorderColor").next() {
            let [r, g, b, a] = color_of(c);
            self.call(wrapper, "SetBackdropBorderColor", (r, g, b, a), dbg);
        }
    }

    /// The `SetBackdrop` table for a `<Backdrop>`. A key the element omits stays absent, so
    /// `SetBackdrop` keeps the ctor's default (tileSize 0, edgeSize 32, insets 0).
    pub(super) fn build_backdrop_table(&self, bd: &Element) -> mlua::Result<Table> {
        let t = self.lua().create_table()?;
        if let Some(f) = bd.attr("bgFile") {
            t.set("bgFile", f)?;
        }
        if let Some(f) = bd.attr("edgeFile") {
            t.set("edgeFile", f)?;
        }
        if bd.attr_bool("tile") {
            t.set("tile", true)?;
        }
        if let Some(v) = children_named(bd, "EdgeSize").next().and_then(abs_value) {
            t.set("edgeSize", v)?;
        }
        if let Some(v) = children_named(bd, "TileSize").next().and_then(abs_value) {
            t.set("tileSize", v)?;
        }
        if let Some(ins) = children_named(bd, "BackgroundInsets").next() {
            let src = children_named(ins, "AbsInset").next().unwrap_or(ins);
            let it = self.lua().create_table()?;
            for k in ["left", "right", "top", "bottom"] {
                if let Some(v) = src.attr(k).and_then(|v| v.trim().parse::<f32>().ok()) {
                    it.set(k, v)?;
                }
            }
            t.set("insets", it)?;
        }
        Ok(t)
    }
}
