use mlua::{ObjectLike, Table};

use crate::framexml::{self, Element};

use super::regions::FontAttrs;
use super::{abs_dim, children_named, Loader};

impl Loader<'_> {
    /// LoadXML attribute handling (`0x769820`): the subset the v1 object model exposes, plus a
    /// warn-once for the attributes whose methods don't exist yet (a next-phase gap, not a workaround).
    pub(super) fn apply_attrs(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        if el.attr_bool("hidden") {
            self.call(wrapper, "Hide", (), dbg);
        }
        // **An unrecognised `frameStrata=` WARNS and skips; it does not raise** (decision 2160).
        //
        // The two doors are genuinely different in the reference and this is the one that is
        // quiet. `CSimpleFrame::LoadXML 0x769820` resolves the name through `0x6f17d0`, whose miss
        // arm returns `eax = 0` without writing the caller's out-parameter (`0x6f17ff`); the miss
        // leg then pushes `0x878658 "Frame %s: Unknown frame strata: %s"` at severity 1 into the
        // document's `CStatus` sink (`0x7699a4 call [edx+0xc]`, which returns — all five `Add`
        // implementations image-wide are raise-free) and **reconverges with the hit path at
        // `0x7699ad`**, carrying straight on into `frameLevel` and the rest of the subtree.
        // `SetFrameStrata 0x76a470` is called only on the hit arm (`0x769971`), so the frame keeps
        // whatever stratum it already had — its template's, its parent's, or the ctor's MEDIUM
        // (`[+0xc0] = 3` at `0x7690c2`).
        //
        // The **Lua** binding is the loud one and stays as it is: `0x774360`'s miss reaches
        // `0x774456 call 0x6f4940` (`luaL_error`), whose callee chain `luaG_errormsg 0x6fc780` /
        // `luaD_throw 0x6f5d80` contains no `ret` at all — the epilogue after it is dead code.
        //
        // Resolving here rather than letting `SetFrameStrata` raise is what reproduces that split:
        // routed through `self.call`, a bad value became a `report.errors` row, which is a script
        // error the player sees — and it cost `EQL3` its whole `EQL3_Log.xml`, on a `<Frame
        // frameStrata="ARTWORK">` (a draw-LAYER name) the real client shrugs at.
        if let Some(strata) = el.attr("frameStrata") {
            if crate::script::object::strata_from_str(strata).is_some() {
                self.call(wrapper, "SetFrameStrata", strata.to_string(), dbg);
            } else {
                self.report
                    .warnings
                    .push(format!("{dbg}: Unknown frame strata: {strata}"));
            }
        }
        if let Some(level) = el.attr("frameLevel") {
            if let Ok(n) = level.parse::<i64>() {
                self.call(wrapper, "SetFrameLevel", n, dbg);
            }
        }
        if let Some(alpha) = el.attr("alpha") {
            if let Ok(a) = alpha.parse::<f32>() {
                self.call(wrapper, "SetAlpha", a, dbg);
            }
        }
        // `scale=` on a MODEL pane is the model's own scale, not the frame's: `CSimpleModel::
        // LoadXML` (`0x76cac0`) writes it into `+0x3a0` at `76cb61` — the field `SetModelScale`
        // writes — and raises `Frame %s: Invalid model scale: %s` at `76cb92` for `≤ 0` (a raise,
        // not a clamp). Every `scale=` in the shipped
        // FrameXML sits on a model pane (the cooldown indicator's 0.75, the autocast shine's
        // 1.2/1.22, the pings' 0.4, the dressing room's 2.0), and until decision 2007 the loader
        // read none of them. Whether the generic frame loader (`0x769820`) reads a `scale`
        // attribute of its own is unconfirmed; a plain frame's `scale=` is left as it was.

        // `file=` on a model pane is `SetModel` (`CSimpleModel::LoadXML` `0x76cac0` installs the
        // file into the widget, resident or streaming — decision 2013). Until 2013 no XML-declared
        // pane ever held a file: the loader read `file=` only to turn the cooldown indicator's
        // pane into a native widget of ours (retired by 2019), and the pings, the shine and the
        // item card were bare panes to the engine.
        let model_kind = super::model_kind_tag(&el.tag);
        if model_kind {
            if let Some(file) = el.attr("file") {
                let text = self.resolve_text(file, dbg);
                self.call(wrapper, "SetModel", text, dbg);
            }
        }
        if let Some(scale) = el.attr("scale") {
            if model_kind {
                match scale.trim().parse::<f32>() {
                    Ok(s) if s > 0.0 => self.call(wrapper, "SetModelScale", s, dbg),
                    _ => self.warn_once(
                        &format!("model-scale:{dbg}"),
                        format!("Frame {dbg}: Invalid model scale: {scale}"),
                    ),
                }
            }
        }
        // The model pane's own fog attributes and `<FogColor>` child (`CSimpleModel::LoadXML`
        // `0x76cac0`). `fogNear`/`fogFar` are **clamped at `≥ 0`** here and only
        // here (`76cbbb`-`76cbd2` / `76cbf3`-`76cc0a`: `0.0 fcomp value ; jne store ; else store
        // 0.0`) — the Lua setters store raw. The `<FogColor>` child writes the packed colour AND
        // arms the fog bit, so it is `SetFogColor` in every respect; nothing in XML touches the
        // light. Decision 2027.
        if model_kind {
            for (attr, verb) in [("fogNear", "SetFogNear"), ("fogFar", "SetFogFar")] {
                if let Some(v) = el.attr(attr).and_then(|v| v.trim().parse::<f32>().ok()) {
                    self.call(wrapper, verb, v.max(0.0), dbg);
                }
            }
            if let Some(fc) = children_named(el, "FogColor").next() {
                let ch = |k: &str, d: f32| {
                    fc.attr(k)
                        .and_then(|v| v.trim().parse::<f32>().ok())
                        .unwrap_or(d)
                };
                self.call(
                    wrapper,
                    "SetFogColor",
                    (ch("r", 0.0), ch("g", 0.0), ch("b", 0.0), ch("a", 1.0)),
                    dbg,
                );
            }
        }
        if let Some(id) = el.attr("id") {
            if let Ok(n) = id.parse::<i64>() {
                self.call(wrapper, "SetID", n, dbg);
            }
        }
        // `clampedToScreen="true"` → SetClampedToScreen (`0x768cc0` — geometry flags bit4,
        // the layout resolve's screen clamp; GameTooltip frames carry it by construction).
        if el.attr_bool("clampedToScreen") {
            self.call(wrapper, "SetClampedToScreen", true, dbg);
        }
        // `enableMouse` lands as the real EnableMouse call (the hit test gates on it —
        // pointer.rs): a ref frame authored mouse-blocking (StaticPopup, BlackoutWorld, the
        // minimap cluster) must actually capture. The loader ignored it for a while after the
        // native landed — the stale "v1 gap" note here — which left those frames click-through
        // and the minimap deaf to its ping click (0434 phase 6c's root cause).
        if el.attr_bool("enableMouse") {
            self.call(wrapper, "EnableMouse", true, dbg);
        }
        // `<HitRectInsets><AbsInset left= right= top= bottom=/></HitRectInsets>` (also accepted
        // inline on the element) → SetHitRectInsets: the frame's MOUSE rect, inset from its
        // resolved rect. Absent sides read 0, so a partial element insets only what it names.
        if let Some(ins) = children_named(el, "HitRectInsets").next() {
            let src = children_named(ins, "AbsInset").next().unwrap_or(ins);
            let side = |k: &str| {
                src.attr(k)
                    .and_then(|v| v.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            self.call(
                wrapper,
                "SetHitRectInsets",
                (side("left"), side("right"), side("top"), side("bottom")),
                dbg,
            );
        }
        // `<ResizeBounds><minResize><AbsDimension x= y=/></minResize><maxResize>…` → the resize
        // quad (`0x769820`'s child-loop arm at `0x769baa`). Three clauses from the reference
        // rather than from the shape of the element:
        //
        //  · **Both pairs are written unconditionally once `<ResizeBounds>` matches**, each side
        //    defaulting to `0` before its lookup — so a block carrying only `<minResize>` RESETS
        //    the max pair to unbounded. That is why this writes both calls, never just the one it
        //    found.
        //  · `x` is the WIDTH and `y` the HEIGHT, through the same logical→internal transform
        //    `<Size>` uses — the same `abs_dim` reader, and the same inline-or-`AbsDimension` pair
        //    of forms.
        //  · `<Size>` and `<Anchors>` are consumed by the base-class `CLayoutFrame::LoadXML`
        //    *before* the child loop is entered at all, so they always precede this structurally
        //    whatever the document order — and neither path clamps, so an authored `<Size>`
        //    outside the authored bounds survives load unchanged.
        //
        // The shipped 1.12.1 FrameXML uses it exactly once (`FloatingChatFrame.xml:223`,
        // min 296×75 / max 608×400) and never calls the Lua setters at all.
        if let Some(rb) = children_named(el, "ResizeBounds").next() {
            for (tag, verb) in [("minResize", "SetMinResize"), ("maxResize", "SetMaxResize")] {
                let (x, y) = children_named(rb, tag).next().map_or((None, None), abs_dim);
                self.call(wrapper, verb, (x.unwrap_or(0.0), y.unwrap_or(0.0)), dbg);
            }
        }
        // `movable`/`resizable` → SetMovable/SetResizable — the same flag word the methods write
        // (`0x76a3c0` with mask 0x100 / 0x200; the XML loader `0x769820` calls that very setter).
        // Both were in the gap list below until the movable family
        // landed; leaving them there would have left every `movable="true"` reference window
        // undraggable while `SetMovable` worked from Lua.
        if el.attr_bool("movable") {
            self.call(wrapper, "SetMovable", true, dbg);
        }
        if el.attr_bool("resizable") {
            self.call(wrapper, "SetResizable", true, dbg);
        }
        // `toplevel` → SetToplevel — the third bit of that same flag word (`0x76a3c0` mask `0x1`,
        // XML site `0x7698ec`), and the third attribute to
        // graduate out of the gap list below for the same reason: the raise law is built
        // (`script::object::toplevel`), so accepting the attribute now means the behaviour, not
        // silence. 82 corpus addons and thirteen of our own frames declare it.
        if el.attr_bool("toplevel") {
            self.call(wrapper, "SetToplevel", true, dbg);
        }
        // `enableKeyboard="true"` — the XML half of the flag, which enables BOTH key kinds
        // (`0x769ae8`/`0x769af3`). The flag is real, and `script::keyboard`'s delivery
        // walk is what reads it (1319).
        if el.attr_bool("enableKeyboard") {
            self.call(wrapper, "EnableKeyboard", true, dbg);
        }
    }

    /// `<Size>` → SetWidth/SetHeight (`0x767800`). Accepts either `<Size><AbsDimension x= y=/>`
    /// or the inline `<Size x= y=/>` form; a dimension that's absent is left untouched (the client's
    /// "0 = derive"). ALL `<Size>` children apply, in document order — template expansion appends the
    /// instance's children after the template's ([`crate::framexml::expand`]), and the client simply
    /// processes each child in turn, so an instance's own `<Size>` overwrites its template's. (Taking
    /// only `.next()` here silently pinned every templated frame to the TEMPLATE's size; caught on
    /// the quest log's Abandon button — 125×21 in the instance XML, 80×22 on screen.)
    /// `<TitleRegion setAllPoints="true"/>` (or one with its own `<Size>`/`<Anchors>`): the
    /// frame's drag handle, built through the same `CreateTitleRegion` the Lua API exposes — so
    /// the element and a later `frame:CreateTitleRegion()` name ONE object (XML site `0x769b2a`
    /// is the same code path as `CreateTitleRegion 0x773910`, idempotent) — then laid out like
    /// any region.
    /// The stock `TutorialFrame.xml` declares one over its whole plate (1976).
    pub(super) fn apply_title_region(
        &mut self,
        el: &Element,
        wrapper: &Table,
        self_name: &str,
        dbg: &str,
    ) {
        let Some(tr) = children_named(el, "TitleRegion").next() else {
            return;
        };
        let region: Table = match wrapper.call_method("CreateTitleRegion", ()) {
            Ok(r) => r,
            Err(e) => {
                self.report
                    .errors
                    .push(format!("{dbg}: CreateTitleRegion: {e}"));
                return;
            }
        };
        self.apply_region_layout(tr, &region, self_name, dbg, FontAttrs::Own);
    }

    pub(super) fn apply_size(&mut self, el: &Element, wrapper: &Table, dbg: &str) {
        for size in children_named(el, "Size") {
            let (x, y) = abs_dim(size);
            if let Some(w) = x {
                self.call(wrapper, "SetWidth", w, dbg);
            }
            if let Some(h) = y {
                self.call(wrapper, "SetHeight", h, dbg);
            }
        }
    }

    /// `<Anchors>` → SetPoint per `<Anchor point= relativeTo= relativePoint=><Offset .../></Anchor>`
    /// (`0x767800`). `relativePoint` defaults to `point`; `relativeTo` is `$parent`-substituted
    /// (`0x76c5b0`) and passed by name (the object model resolves it, falling back to the parent
    /// when absent/unresolved); a missing `point` is skipped with a warning ("Invalid anchor
    /// point").
    /// The `setAllPoints="true"` shorthand applies first, like the region path — a frame carrying
    /// only the attribute (WorldMapFrame's chrome layers) pins TOPLEFT+BOTTOMRIGHT to its parent.
    pub(super) fn apply_anchors(
        &mut self,
        el: &Element,
        wrapper: &Table,
        parent_name: &str,
        dbg: &str,
    ) {
        if el.attr_bool("setAllPoints") {
            self.call(wrapper, "SetAllPoints", (), dbg);
        }
        for anchors in children_named(el, "Anchors") {
            for anchor in children_named(anchors, "Anchor") {
                let Some(point) = anchor.attr("point") else {
                    self.report
                        .warnings
                        .push(format!("{dbg}: <Anchor> without a point; skipped"));
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
                    region: false,
                    args,
                    dbg: dbg.to_string(),
                };
                // The XML path's own law, which resolves the name itself and defers a target
                // the enclosing frame's subtree has not built yet (`Loader::apply_anchor`).
                self.apply_anchor(d, true);
            }
        }
    }
}
