use mlua::Table;

use crate::framexml::Element;

use super::{children_named, color_of, Loader};

impl Loader<'_> {
    /// One `<Backdrop>` (LoadXML `0x77e6c0`): the table Lua `SetBackdrop` takes, then the two
    /// colours. Each element builds a fresh struct and installs it (`0x769c54`-`0x769c7a`), so a
    /// frame's own `<Backdrop>` replaces its template's. A present `<Color>` defaults a missing
    /// channel to black, alpha 1 (`0x6f23d0`), not the white a table `SetBackdrop` leaves.
    pub(super) fn apply_backdrop(&mut self, bd: &Element, wrapper: &Table, dbg: &str) {
        let table = match self.build_backdrop_table(bd) {
            Ok(t) => t,
            Err(e) => {
                self.report.fault(format!("{dbg}: <Backdrop>: {e}"));
                return;
            }
        };
        self.call(wrapper, "SetBackdrop", table, dbg);
        if let Some(c) = children_named(bd, "Color").next() {
            self.call(wrapper, "SetBackdropColor", color_of(c).fractions(), dbg);
        }
        if let Some(c) = children_named(bd, "BorderColor").next() {
            self.call(
                wrapper,
                "SetBackdropBorderColor",
                color_of(c).fractions(),
                dbg,
            );
        }
    }

    /// The `SetBackdrop` table for a `<Backdrop>`. A key the element omits stays absent, so
    /// `SetBackdrop` keeps the ctor's default (tileSize 0, edgeSize 32, insets 0).
    pub(super) fn build_backdrop_table(&mut self, bd: &Element) -> mlua::Result<Table> {
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
        // Each length is set only when its reader succeeds (`0x77e779`, `0x77e7ae`, `0x77e7ef`).
        if let Some(v) = children_named(bd, "EdgeSize")
            .next()
            .and_then(|e| self.value_read(e))
        {
            t.set("edgeSize", v)?;
        }
        if let Some(v) = children_named(bd, "TileSize")
            .next()
            .and_then(|e| self.value_read(e))
        {
            t.set("tileSize", v)?;
        }
        if let Some([l, r, top, b]) = children_named(bd, "BackgroundInsets")
            .next()
            .and_then(|e| self.inset(e))
        {
            let it = self.lua().create_table()?;
            for (k, v) in [("left", l), ("right", r), ("top", top), ("bottom", b)] {
                it.set(k, v)?;
            }
            t.set("insets", it)?;
        }
        Ok(t)
    }
}
