use mlua::Table;

use crate::framexml::Element;

use super::{color_of, Loader};

impl Loader<'_> {
    /// One `<Backdrop>` (LoadXML `0x77e6c0`): the table Lua `SetBackdrop` takes, then the two
    /// colours. Each element builds a fresh struct and installs it (`0x769c54`-`0x769c7a`), so a
    /// frame's own `<Backdrop>` replaces its template's. A present `<Color>` defaults a missing
    /// channel to black, alpha 1 (`0x6f23d0`), not the white a table `SetBackdrop` leaves.
    pub(super) fn apply_backdrop(&mut self, bd: &Element, wrapper: &Table, dbg: &str) {
        let parts = self.backdrop_children(bd);
        let table = match self.build_backdrop_table(bd, &parts) {
            Ok(t) => t,
            Err(e) => {
                self.report.fault(format!("{dbg}: <Backdrop>: {e}"));
                return;
            }
        };
        self.call(wrapper, "SetBackdrop", table, dbg);
        crate::script::xml_layout::backdrop(
            self.lua(),
            wrapper,
            parts.edge_size,
            parts.tile_size,
            parts.insets,
        );
        if let Some(c) = parts.color {
            self.call(wrapper, "SetBackdropColor", c.fractions(), dbg);
        }
        if let Some(c) = parts.border_color {
            self.call(wrapper, "SetBackdropBorderColor", c.fractions(), dbg);
        }
    }

    /// The Backdrop's child loop (`0x77e757`-`0x77e895`): one dispatch in document order, each
    /// child of a kind overwriting the last, a length or inset only when its read succeeds
    /// (`0x77e779`, `0x77e7ae`, `0x77e7ef`), and any other child logged as
    /// `Unknown child node in %s element: %s` (`0x871464`, `0x77e875`-`0x77e88a`).
    fn backdrop_children(&mut self, bd: &Element) -> BackdropChildren {
        let mut out = BackdropChildren::default();
        for child in &bd.children {
            let is = |t: &str| child.tag.eq_ignore_ascii_case(t);
            if is("TileSize") {
                if let Some(v) = self.value_read(child) {
                    out.tile_size = Some(v);
                }
            } else if is("EdgeSize") {
                if let Some(v) = self.value_read(child) {
                    out.edge_size = Some(v);
                }
            } else if is("BackgroundInsets") {
                if let Some(v) = self.inset(child) {
                    out.insets = Some(v);
                }
            } else if is("Color") {
                out.color = Some(color_of(child));
            } else if is("BorderColor") {
                out.border_color = Some(color_of(child));
            } else {
                let line = format!("Unknown child node in {} element: {}", bd.tag, child.tag);
                self.status_warning(line);
            }
        }
        out
    }

    /// The `SetBackdrop` table for a `<Backdrop>` and its children. A key the element omits stays
    /// absent, so `SetBackdrop` keeps the ctor's default (tileSize 0, edgeSize 32, insets 0).
    fn build_backdrop_table(
        &mut self,
        bd: &Element,
        parts: &BackdropChildren,
    ) -> mlua::Result<Table> {
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
        if let Some(v) = parts.edge_size {
            t.set("edgeSize", v.script(self.model().layout_units()))?;
        }
        if let Some(v) = parts.tile_size {
            t.set("tileSize", v.script(self.model().layout_units()))?;
        }
        if let Some(i) = parts.insets {
            let [l, r, top, b] = self.script_of(i);
            let it = self.lua().create_table()?;
            for (k, v) in [("left", l), ("right", r), ("top", top), ("bottom", b)] {
                it.set(k, v)?;
            }
            t.set("insets", it)?;
        }
        Ok(t)
    }
}

/// What a `<Backdrop>`'s children set, the last of each kind.
#[derive(Default)]
struct BackdropChildren {
    tile_size: Option<crate::layout::Len>,
    edge_size: Option<crate::layout::Len>,
    insets: Option<[crate::layout::Len; 4]>,
    color: Option<crate::color::Rgba8>,
    border_color: Option<crate::color::Rgba8>,
}
