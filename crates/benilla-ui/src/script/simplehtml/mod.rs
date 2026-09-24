//! The **`SimpleHTML` widget** (`CSimpleHTML`, class size `0x374`, ctor `0x789dd0`) — its markup
//! engine, its block layout, its four element fonts, and its 19-entry Lua method table.
//!
//! Ground truth: byte-verified against `WoW.exe`. The parse itself is [`parse`]; this module is
//! the half that touches the model — turning a block list into real FontString/Texture regions
//! anchored bottom-to-top, and exposing the widget's Lua surface.
//!
//! ## What the widget is, in one paragraph
//!
//! `SetText(s)` throws away every block of the previous parse, runs `s` through a **strict XML**
//! parser, and — if and only if the document is well-formed, rooted at `HTML`, and has a `BODY`
//! child — walks that BODY emitting one block per `<H1>`/`<H2>`/`<H3>`/`<P>`/`<BR/>`/`<IMG>`.
//! Anything else falls to the plain-text path: the **raw** string as one `P` block, left-aligned,
//! whitespace intact. Each text block is one `CSimpleFontString` at the frame's declared width
//! with **no height set**, so its rect is the intrinsic height of the wrapped text; block 0 hangs
//! from the frame's TOPLEFT and block *N* from block *N−1*'s BOTTOMLEFT at `−spacing`. Spacing
//! defaults to 0, so blocks are flush.
//!
//! ## Three defaults a re-implementation gets wrong, and this one does not
//!
//! - **Block justifyH is LEFT**, never the `CSimpleFontString` ctor's CENTER: `align` is
//!   pre-loaded with `1` before the attribute is read (`0x78a7c8`) and `0x78ae78` writes it into
//!   the block after `SetFontObject`, so the element font's own `justifyH` can never reappear.
//! - **Element fonts start with no font at all**, and `H1`/`H2`/`H3` fall back to `P`'s whenever
//!   their own resolved path is empty (`0x78ae30` measures `SStr::Length(&font->path)`). **Nothing
//!   in this TU scales a header** — no height multiply, no size table — so an `<H1>` under a
//!   single `<FontString>` declaration renders at exactly the `<P>` size. That is the reference
//!   `ItemTextFrame` render, not an approximation of it.
//! - **`GetContentHeight` does not exist in 1.12.1.** The method table has no height getter and
//!   nothing in the TU aggregates one; the hosting ScrollFrame measures the subtree generically
//!   instead (`0x786e30`'s subtree union), which is what ours does too — the blocks are real
//!   regions of the SimpleHTML, so `GetVerticalScrollRange` sees them without SimpleHTML
//!   publishing anything.
//!
//! ## Where this diverges from the reference, deliberately
//!
//! - **`nextYOffset` is not pixel-snapped.** The reference stores
//!   `−pixelSnap(spacing)` (`0x766750`, quantising to whole *device* pixels). Our layout is in
//!   logical units and the solver has no device-pixel quantiser to reach for; at the default
//!   spacing of 0 the two agree exactly, and at any other value they differ by under a pixel.
//! - **Intra-block line spacing is not modelled**, because nothing in this engine models line
//!   spacing at all (see `script::font_block`'s own note on the withheld `Set/GetSpacing`). What
//!   `SimpleHTML:SetSpacing` *does* honour here is the half SimpleHTML itself owns and computes:
//!   the inter-block step. That is a real, drawn effect rather than a stored-and-ignored number,
//!   which is why this pair ships where the FontString/Font ones do not.
//! - **A block's wrap width of 0** (a SimpleHTML sized only by anchors) lets the measure
//!   round-trip supply the natural text width, where the reference leaves the block at width 0.
//!   That is this engine's standing law for every height-less/width-less FontString, and inverting
//!   it here would make a SimpleHTML the one widget whose text vanishes when it is anchor-sized.
//!
//! ## The Lua table, and the one name that is NOT on it
//!
//! The method table `.data 0x87ba80` dumps as **19 `{name, fn}` pairs**, and all 19 are installed
//! here. The first sixteen take an **optional leading element-name string** resolved by
//! `0x795d80`; the last
//! three (`SetText`, `SetHyperlinkFormat`, `GetHyperlinkFormat`) do not.
//!
//! **There is no `GetText`.** Later clients grew one; build 5875's table does not have it, and
//! adding a name a table does not carry is exactly as wrong as missing one (`script::font_block`'s
//! module doc states the same law for the font block). A caller that wants the source string keeps
//! it — the reference's own `ItemTextFrame` calls `ItemTextGetText()`.

use std::collections::HashMap;

use mlua::{FromLua, Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use super::{FontObject, FontShadow, Model, Outline, RegionData};
use crate::justify::{self, Justify};
use crate::layout::{Anchor, Point};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, FrameKind, RegionHandle, RegionKind};

mod parse;

pub(crate) use parse::Block;
use parse::{ELEMENT_NAMES, ELEM_P};

/// Registry key of the SimpleHTML method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_SIMPLEHTML_METHODS: &str = "__benilla_simplehtml_methods";

// ─────────────────────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// One of the four **element fonts** (`CSimpleHTML+0x350`…`+0x35c`, index `0 = P, 1 = H1, 2 = H2,
/// 3 = H3`), each a `0x80`-byte subclass of `CSimpleFont` the ctor loop `0x789e61`–`0x789ea3`
/// allocates.
///
/// Modelled as "the named object it points at, plus the properties it set for itself" rather than
/// as a flattened snapshot, so a later `GameFontNormal:SetFont(…)` reaches the *next* parse — the
/// live `parentFontObject` link the real `CFontInstance` carries. The explicit mask is the same
/// `FONTINSTANCE+0x038` severance record a region keeps ([`super::FontExplicit`]); reusing it is
/// what lets a block inherit the mask and stay correct under `script::font::propagate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ElementFont {
    /// `inherits=` / `SetFontObject(elem, obj)` — the object this element resolves through.
    pub(crate) font_object: Option<String>,
    /// Which properties this element set for **itself** (and so must survive a re-read of the
    /// object above).
    pub(crate) explicit: super::FontExplicit,
    /// `FieldBlock.path +0x3c`, ctor `""` — the field `0x78ae30` measures for the H1→P fallback.
    pub(crate) font_path: Option<String>,
    /// `FieldBlock` height.
    pub(crate) font_height: Option<f32>,
    /// `SetFont`'s flags / XML `outline=`.
    pub(crate) outline: Outline,
    /// `SetTextColor` / `<Color>`.
    pub(crate) color: Option<[f32; 4]>,
    /// `SetShadowColor`/`SetShadowOffset` / `<Shadow>`.
    pub(crate) shadow: Option<FontShadow>,
    /// `CSimpleFont+0x54`, ctor `0x212` (`0x783a98`) — CENTER | MIDDLE | the one-to-one bit.
    pub(crate) justify: Justify,
    /// `FieldBlock.spacing +0x50`, ctor **0** (`0x783a81`). Both the inter-block step
    /// (`nextYOffset = −pixelSnap(spacing)`) and, in the reference, the intra-block line gap.
    pub(crate) spacing: f32,
}

/// A `SimpleHTML` frame's runtime state — the members `CSimpleHTML` adds over `CSimpleFrame`.
///
/// Held in a [`Model`]-side map rather than in [`crate::widget::KindState`] for the reason
/// `Model::backdrops` is: its contents are script-layer types ([`FontObject`], [`FontShadow`],
/// [`Outline`]), and `widget::kinds` is the layer *below* `script` and does not depend on it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SimpleHtmlState {
    /// `+0x350`…`+0x35c`.
    pub(crate) fonts: [ElementFont; 4],
    /// `+0x360`, ctor `"|H%s|h%s|h"` (`0x87a838`).
    pub(crate) hyperlink_format: String,
    /// The CONTENTNODE list `+0x340` — every region the last `SetText` built, in emission order,
    /// text blocks and images alike. This is the free list the next `SetText` walks.
    pub(crate) blocks: Vec<RegionHandle>,
}

impl Default for SimpleHtmlState {
    fn default() -> Self {
        SimpleHtmlState {
            fonts: Default::default(),
            hyperlink_format: parse::DEFAULT_HYPERLINK_FORMAT.to_string(),
            blocks: Vec::new(),
        }
    }
}

/// The paint one block copies out of its element font — the result of `CFontInstance::
/// SetFontObject 0x770c60`, which forces all five property groups changed (`0x770d06 or …,0x1F`)
/// and pulls immediately, with the element's own explicit sets layered back on top.
#[derive(Clone, Debug, Default)]
struct BlockPaint {
    font_object: Option<String>,
    explicit: super::FontExplicit,
    font_path: Option<String>,
    font_height: Option<f32>,
    outline: Outline,
    color: Option<[f32; 4]>,
    shadow: Option<FontShadow>,
    justify: Justify,
    spacing: f32,
}

/// Flatten one element font against the (live) font object it inherits.
fn resolve_font(model: &Model, ef: &ElementFont) -> BlockPaint {
    let fo: Option<FontObject> = ef
        .font_object
        .as_deref()
        .and_then(|n| model.font_object(n))
        .cloned();
    let mut p = BlockPaint {
        font_object: ef.font_object.clone(),
        explicit: ef.explicit,
        justify: Justify::default(),
        spacing: ef.spacing,
        ..BlockPaint::default()
    };
    if let Some(fo) = &fo {
        p.font_path = fo.font.clone();
        p.font_height = fo.height;
        p.outline = fo.outline;
        p.color = fo.color;
        p.shadow = fo.shadow;
        if let Some(j) = fo.justify_h {
            p.justify.set_h(j);
        }
        if let Some(j) = fo.justify_v {
            p.justify.set_v(j);
        }
    }
    if ef.explicit.face {
        p.font_path = ef.font_path.clone();
    }
    if ef.explicit.height {
        p.font_height = ef.font_height;
    }
    if ef.explicit.outline {
        p.outline = ef.outline;
    }
    if ef.explicit.color {
        p.color = ef.color;
    }
    if ef.explicit.shadow {
        p.shadow = ef.shadow;
    }
    if ef.explicit.justify_h {
        p.justify.0 = justify::set_axis(p.justify.0, justify::H_MASK, ef.justify.0);
    }
    if ef.explicit.justify_v {
        p.justify.0 = justify::set_axis(p.justify.0, justify::V_MASK, ef.justify.0);
    }
    p
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// SetText — the rebuild
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `CSimpleHTML::SetText 0x78a3a0`, whole: free the old blocks, parse, and build the new ones.
///
/// Returns `usedMarkup` (`0x78a519`'s `al`) — the Lua shim discards it, our tests do not.
pub(crate) fn set_text(model: &mut Model, fh: FrameHandle, raw: &str) -> bool {
    // `0x78a3a0`'s first step. The reference pool-frees the previous parse's widgets; ours must
    // too, or a second `SetText` leaves the first one's FontStrings standing behind the new
    // ones — the failure a book with a "next page" button hits on its very first turn.
    let old = model
        .simple_html
        .get_mut(&fh)
        .map(|s| std::mem::take(&mut s.blocks))
        .unwrap_or_default();
    for rh in old {
        free_block(model, rh);
    }

    let (frame_name, hyperlink_format) = {
        let name = model
            .arena
            .frame(fh)
            .and_then(|f| f.name.clone())
            .unwrap_or_default();
        let fmt = model
            .simple_html
            .get(&fh)
            .map(|s| s.hyperlink_format.clone())
            .unwrap_or_else(|| parse::DEFAULT_HYPERLINK_FORMAT.to_string());
        (name, fmt)
    };
    let parsed = parse::parse_markup(raw, &frame_name, &hyperlink_format);
    // The reference pushes these through the frame's error sink to the console. Ours are host
    // warnings: a malformed page is the player's content being wrong, not a script raising.
    for e in &parsed.errors {
        model.record_warning(e.clone());
    }

    build(model, fh, &parsed.blocks);
    parsed.used_markup
}

/// `AddTextBlock 0x78adb0`/the `<IMG>` handler `0x78ab40`'s loop — the anchor chain, the fonts,
/// and the `nextY` bookkeeping.
fn build(model: &mut Model, fh: FrameHandle, blocks: &[Block]) {
    let frame_id = model.frame_id(fh);
    // `0x78ae19 call [CLayoutFrame vtbl + 0x1c]` = `0x768420`, `fld [ecx+0x50]` — the frame's
    // **declared** width, not its resolved rect. Every block is exactly that wide, which is what
    // makes each one word-wrap at the frame width; `OnRectChanged 0x78a320`'s corollary is that a
    // later resize does NOT re-wrap, because each block snapshotted the width at creation.
    let width = model
        .layout_inputs
        .get(&fh)
        .map_or(0.0, |input| input.width);

    // `+0x348 prevBlock` — the last **text** block, and `+0x34c nextYOffset` (always <= 0).
    let mut prev_block: Option<u32> = None;
    let mut next_y = 0.0f32;
    let mut made: Vec<RegionHandle> = Vec::with_capacity(blocks.len());

    for block in blocks {
        match block {
            Block::Text { text, elem, align } => {
                // The empty-path fallback (`0x78ae29`–`0x78ae54`): `elementFont[elem]`, unless its
                // resolved path is empty, in which case `elementFont[0]` — so a declaration that
                // supplies only `<FontString>` supplies the font for every element, headers
                // included.
                let paint = {
                    let st = model.simple_html.entry(fh).or_default().clone();
                    let own = resolve_font(model, &st.fonts[*elem]);
                    if own.font_path.as_deref().unwrap_or("").is_empty() {
                        resolve_font(model, &st.fonts[ELEM_P])
                    } else {
                        own
                    }
                };
                let Some(rh) =
                    model
                        .arena
                        .create_region(fh, RegionKind::FontString, DrawLayer::Artwork, 0)
                else {
                    // The frame died under us — unreachable (we hold its handle and just made
                    // regions on it), but stop rather than orphan what is already built: the
                    // blocks so far are still recorded below, so the next `SetText` frees them.
                    break;
                };
                let id = model.region_id(rh);
                let mut d = RegionData {
                    anchors: vec![anchor_for(prev_block, frame_id, Point::TopLeft, next_y)],
                    // `SetWidth(frame.GetWidth())` and **no height** — the block's rect height is
                    // the FontString's intrinsic wrapped height, which the engine's measure
                    // round-trip supplies exactly as the client's font engine supplies
                    // `0x7729b0` → `0x5c2070`.
                    size: Some((width, 0.0)),
                    text: Some(text.clone()),
                    font_object: paint.font_object.clone(),
                    font_explicit: paint.explicit,
                    font_path: paint.font_path.clone(),
                    font_height: paint.font_height,
                    outline: paint.outline,
                    vertex_color: paint.color,
                    font_shadow: paint.shadow,
                    ..RegionData::default()
                };
                // `block->+0x120 = (block->+0x120 & ~7) | (align & 7)` (`0x78ae78`), written
                // AFTER `SetFontObject`, so the tag's `align` always wins over the element font's
                // own justifyH. justifyV is left alone and keeps whatever the element font
                // supplied (ctor MIDDLE) — visually inert here, because the block's rect height IS
                // its text height.
                d.justify = Justify(justify::set_axis(paint.justify.0, justify::H_MASK, *align));
                // …and it can never be RE-inherited: `0x78ae68` clears the per-bit justifyH
                // inherit mask (`+0x124 &= ~7`) and `0x78ae95` the whole CFontInstance justify
                // group bit (`+0xd4 &= ~2`). Marking both axes explicit is what that pair means
                // here — it is what stops `script::font::propagate` from re-centering a built
                // block the moment somebody mutates the font object it drew from.
                d.font_explicit.justify_h = true;
                d.font_explicit.justify_v = true;
                model.region_data.insert(rh, d);
                model.touch_measure(rh); // a text block arrives text-in-hand
                prev_block = Some(id);
                next_y = -paint.spacing;
                made.push(rh);
            }
            Block::Image {
                src,
                width: w,
                height: h,
                align,
                floated,
            } => {
                let Some(rh) =
                    model
                        .arena
                        .create_region(fh, RegionKind::Texture, DrawLayer::Artwork, 0)
                else {
                    break; // as above
                };
                // Minted even though nothing anchors TO an image (`prevBlock` is never written by
                // the image path): the id is what the resolve seats the region's rect under, and
                // what `free_block` unmaps on the next `SetText`.
                model.region_id(rh);
                // The anchor corner is selected by `align`, and 8/16/32 get **no
                // `SetPoint` at all** (`0x78ac61 jne 0x78ace8`). An anchorless region never
                // resolves here either, so it draws nothing, which is the same outcome.
                let point = match *align {
                    parse::ALIGN_LEFT => Some(Point::TopLeft),
                    parse::ALIGN_CENTER => Some(Point::Top),
                    parse::ALIGN_RIGHT => Some(Point::TopRight),
                    _ => None,
                };
                let anchors = point
                    .map(|p| vec![anchor_for(prev_block, frame_id, p, next_y)])
                    .unwrap_or_default();
                model.region_data.insert(
                    rh,
                    RegionData {
                        anchors,
                        size: Some((*w, *h)),
                        texture: src.clone(),
                        ..RegionData::default()
                    },
                );
                // An unfloated image reserves its own height in the flow (`0x78ad07`–`0x78ad1b`),
                // and `prevBlock` is **never** written by the image path (three reads of `+0x348`
                // in the whole function, zero writes) — the next block still hangs off the last
                // TEXT block, only lower.
                //
                // The height it reserves is `texture.GetHeight()`, which is `CSimpleTexture`'s
                // **override**: a `<IMG>` with no `height=` reserves its art's own texel height,
                // not nothing (one texel is one FrameXML unit). Asked of the same
                // host oracle the resolve derives the region's rect from, so the space the flow
                // leaves and the space the image fills are one number.
                if !*floated {
                    let reserved = if *h == 0.0 {
                        src.as_deref()
                            .and_then(|p| {
                                super::layout::texel_span(model.texture_size_probe.as_ref(), p)
                            })
                            .map_or(0.0, |(_, th)| th)
                    } else {
                        *h
                    };
                    next_y -= reserved;
                }
                made.push(rh);
            }
        }
    }

    model.simple_html.entry(fh).or_default().blocks = made;
    model.touch_layout();
}

/// The two anchor forms of `AddTextBlock 0x78adb0`'s `SetPoint 0x767c70` step: block 0 pins to the
/// frame, block *N* to its predecessor's bottom edge at `nextYOffset` (negative, i.e. downward).
fn anchor_for(prev_block: Option<u32>, frame_id: u32, point: Point, next_y: f32) -> Anchor {
    match prev_block {
        None => Anchor::new(point, frame_id, point, 0.0, 0.0),
        Some(prev) => {
            let rel = match point {
                Point::TopLeft => Point::BottomLeft,
                Point::Top => Point::Bottom,
                Point::TopRight => Point::BottomRight,
                other => other,
            };
            Anchor::new(point, prev, rel, 0.0, next_y)
        }
    }
}

/// Free one block region — [`crate::script::region::free_region`], which is where this function's
/// body went when the button's label swap needed the same five-map teardown. Kept as a name
/// because `set_text`'s call sites read better with it, and because the doc that explains *why* a
/// destroyed region must answer stale rather than linger lives on the shared law now.
fn free_block(model: &mut Model, rh: RegionHandle) {
    crate::script::region::free_region(model, rh);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The loader's seam
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Apply the font parts an **XML** `<FontString>`/`<FontStringHeaderN>` child of a `<SimpleHTML>`
/// supplies — `font=`, `<FontHeight>`, `outline=` — any subset of which may be absent.
///
/// The region-side twin of this is [`super::apply_font_parts`], and it exists for the same reason:
/// the reference applies XML font attributes in C++ (`CSimpleFont::LoadXML 0x783c30`, reached from
/// `0x78a1fe`…`0x78a26e`), never through the Lua `SetFont`, whose usage string *requires* both a
/// path and a height. Routing the loader through the binding is what forces the binding to be
/// lenient; keeping them separate lets each be right.
pub(crate) fn apply_element_font_parts(
    lua: &Lua,
    this: &Table,
    elem: usize,
    path: Option<String>,
    height: Option<f32>,
    flags: Option<String>,
) -> mlua::Result<()> {
    let fh = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let ef = &mut model.simple_html.entry(fh).or_default().fonts[elem];
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        ef.font_path = Some(p);
        ef.explicit.face = true;
    }
    if let Some(h) = height {
        ef.font_height = Some(h);
        ef.explicit.height = true;
    }
    if let Some(f) = flags {
        ef.outline = Outline::parse(&f);
        ef.explicit.outline = true;
    }
    Ok(())
}

/// The element index an XML child tag names: `<FontString>` → `P`, `<FontStringHeader1|2|3>` →
/// `H1`/`H2`/`H3` (`0x8786cc`, `0x87a8b4`/`0x87a8a0`/`0x87a88c`; assigned at `0x78a1fe`…`0x78a26e`).
pub(crate) fn element_of_xml_tag(tag: &str) -> Option<usize> {
    if tag.eq_ignore_ascii_case("FontString") {
        Some(0)
    } else if tag.eq_ignore_ascii_case("FontStringHeader1") {
        Some(1)
    } else if tag.eq_ignore_ascii_case("FontStringHeader2") {
        Some(2)
    } else if tag.eq_ignore_ascii_case("FontStringHeader3") {
        Some(3)
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The Lua surface
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Resolve `this` to a live `SimpleHTML` frame. Reached through the kind dispatcher, so the guard
/// only fires when a caller fishes the method table out and misapplies it — the same posture
/// `script::scrollframe`'s `with_scroll` takes.
fn html_handle(lua: &Lua, this: &Table) -> mlua::Result<FrameHandle> {
    let h = frame_handle_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model");
    match model.arena.frame(h).map(|f| f.kind) {
        Some(FrameKind::SimpleHtml) => Ok(h),
        _ => Err(mlua::Error::runtime("not a SimpleHTML")),
    }
}

/// `0x795d80` — the **optional leading element-name string**, consumed by the first sixteen
/// methods.
///
/// ```text
/// if (lua_type(L, 2) != LUA_TSTRING)      return 0;          ; 0x795d91
/// if (SStrCmpI(s,"P" )==0) { lua_remove(L,2); return 0; }    ; 0x795db3
/// if (SStrCmpI(s,"H1")==0) { lua_remove(L,2); return 1; }    ; 0x795ddb
/// if (SStrCmpI(s,"H2")==0) { lua_remove(L,2); return 2; }    ; 0x795e06
/// if (SStrCmpI(s,"H3")==0) { lua_remove(L,2); return 3; }    ; 0x795e2e
/// return 0;                                                   ; 0x795e50 (NO lua_remove)
/// ```
///
/// Two silent behaviours fall out and are reproduced exactly: omitting the name addresses **`P`**,
/// and a *string* first argument that is not one of the four is **not** an error and is **not**
/// removed — it stays and is consumed as the shared implementation's first real argument, so
/// `SetFont("h4", path, 15)` targets `P` and hands `"h4"` to `SetFont` as the path.
fn take_element(args: &mut MultiValue) -> usize {
    let matched = match args.front() {
        Some(Value::String(s)) => s.to_str().ok().and_then(|s| {
            ELEMENT_NAMES
                .iter()
                .position(|e| s.as_ref().eq_ignore_ascii_case(e))
        }),
        _ => None,
    };
    match matched {
        Some(i) => {
            args.pop_front();
            i
        }
        None => ELEM_P,
    }
}

/// Read argument `i` of the post-element list, or `nil`.
fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

/// Run `f` over one element font under a short write borrow, then dirty the layout — every one of
/// these can change what the *next* `SetText` draws with, and several change nothing until then.
fn edit_font<T>(
    lua: &Lua,
    this: &Table,
    elem: usize,
    f: impl FnOnce(&mut ElementFont) -> T,
) -> mlua::Result<T> {
    let h = html_handle(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    Ok(f(&mut model.simple_html.entry(h).or_default().fonts[elem]))
}

/// Read one element font's **resolved** paint (the object it inherits, with its own sets on top) —
/// what the getters answer and what a block would be built with.
fn read_font(lua: &Lua, this: &Table, elem: usize) -> mlua::Result<BlockPaint> {
    let h = html_handle(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model");
    let st = model.simple_html.get(&h);
    Ok(match st {
        Some(st) => resolve_font(&model, &st.fonts[elem]),
        None => resolve_font(&model, &ElementFont::default()),
    })
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // ── 0/1 · the font object, and the live link to it ──────────────────────────────────────
    // `SetFontObject([element,] font | "font" | nil)` → 0 values. Unlike a region's, this cannot
    // repaint anything already on screen: the element fonts are prototypes, and only the next
    // `SetText` copies them into blocks (a built block is never re-fonted, `0x795d3e`).
    m.set(
        "SetFontObject",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let name = super::font::resolve("SetFontObject", &arg(&rest, 0))?;
            if let Some(n) = &name {
                let model = lua.app_data_ref::<Model>().expect("model");
                if model.font_object(n).is_none() {
                    return Err(mlua::Error::runtime(format!(
                        "SetFontObject: no font object named '{n}' is registered"
                    )));
                }
            }
            // The **nil form severs the link and leaves the paint standing** — the reference
            // stores a null parent and nothing re-reads or clears the resolved values. A region
            // gets that for free (its paint was copied into `RegionData`); an element font
            // resolves its object lazily, so the standing paint has to be pinned down here or the
            // sever would blank the element instead.
            let standing = name
                .is_none()
                .then(|| read_font(lua, &this, elem))
                .transpose()?;
            edit_font(lua, &this, elem, |ef| {
                if let Some(p) = standing {
                    ef.font_path = p.font_path;
                    ef.font_height = p.font_height;
                    ef.outline = p.outline;
                    ef.color = p.color;
                    ef.shadow = p.shadow;
                    ef.justify = p.justify;
                    ef.explicit = super::FontExplicit {
                        face: ef.font_path.is_some(),
                        height: ef.font_height.is_some(),
                        outline: true,
                        color: ef.color.is_some(),
                        shadow: ef.shadow.is_some(),
                        justify_h: true,
                        justify_v: true,
                    };
                }
                // The severance mask is deliberately **not** reset on a re-point — true for
                // the region side too (`script::font_block`'s `SetFontObject`): the real "stop
                // inheriting this property" signal is a CLEARED bit in the inheritMask
                // (`FONTINSTANCE+0x2c`), cleared by each local setter and never restored — so a
                // property this element set for itself stays severed across a later
                // `SetFontObject`. The two sides of one law must not disagree.
                ef.font_object = name;
            })
        })?,
    )?;
    m.set(
        "GetFontObject",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let h = html_handle(lua, &this)?;
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model
                    .simple_html
                    .get(&h)
                    .and_then(|s| s.fonts[elem].font_object.clone())
                    .filter(|n| model.font_object(n).is_some())
            };
            match name {
                Some(n) => Ok(Value::Table(super::font::wrapper(lua, &n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ── 2/3 · the face ──────────────────────────────────────────────────────────────────────
    // `SetFont([element,] file, height [, flags])` → the NUMBER 1, or nil on an empty path (the
    // load-failure probe shape); the argument gate is the shared `0x79f210` entry, so a numeric
    // string is accepted for either and anything else raises `0x87c69c`'s usage string.
    m.set(
        "SetFont",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let (path, height) =
                super::font_block::set_font_args(&arg(&rest, 0), &arg(&rest, 1), "SimpleHTML")?;
            let flags = match arg(&rest, 2) {
                Value::String(s) => Some(s.to_str()?.to_string()),
                _ => None,
            };
            let ok = !path.is_empty();
            edit_font(lua, &this, elem, |ef| {
                if ok {
                    ef.font_path = Some(path);
                    ef.explicit.face = true;
                }
                ef.font_height = Some(height);
                ef.explicit.height = true;
                if let Some(f) = flags {
                    // The **Lua** flags spelling ("OUTLINE"/"THICKOUTLINE"), a different
                    // vocabulary from the XML `outline=` attribute — see [`Outline::flags`].
                    ef.outline = Outline::flags(&f);
                    ef.explicit.outline = true;
                }
            })?;
            Ok(if ok { Value::Number(1.0) } else { Value::Nil })
        })?,
    )?;
    // `GetFont([element])` → 3 values: path, height, flags string (`""`, never nil).
    m.set(
        "GetFont",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let p = read_font(lua, &this, elem)?;
            let path = match p.font_path {
                Some(path) => Value::String(lua.create_string(&path)?),
                None => Value::Nil,
            };
            Ok((path, p.font_height, p.outline.as_str()))
        })?,
    )?;

    // ── 4/5 · the text colour ───────────────────────────────────────────────────────────────
    m.set(
        "SetTextColor",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let c = rgba(lua, &rest)?;
            edit_font(lua, &this, elem, |ef| {
                ef.color = Some(c);
                ef.explicit.color = true;
            })
        })?,
    )?;
    m.set(
        "GetTextColor",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let c = read_font(lua, &this, elem)?
                .color
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    // ── 6..9 · the shadow ───────────────────────────────────────────────────────────────────
    // `GetShadowColor` returns **four** values (`0x79f9b3 mov eax,0x4`), `GetShadowOffset` two.
    // Either half may be set before the other, so each starts from whatever is already there.
    m.set(
        "SetShadowColor",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let c = rgba(lua, &rest)?;
            edit_font(lua, &this, elem, |ef| {
                let offset = ef.shadow.map_or([0.0, 0.0], |s| s.offset);
                ef.shadow = Some(FontShadow { offset, color: c });
                ef.explicit.shadow = true;
            })
        })?,
    )?;
    m.set(
        "GetShadowColor",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let c = read_font(lua, &this, elem)?
                .shadow
                .map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    // Both arguments are required — the shared impl raises `Usage: %s:SetShadowOffset(x, y)`
    // (`0x87c6e8`) rather than defaulting the missing one.
    m.set(
        "SetShadowOffset",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let x = f32::from_lua(arg(&rest, 0), lua)?;
            let y = f32::from_lua(arg(&rest, 1), lua)?;
            edit_font(lua, &this, elem, |ef| {
                let color = ef.shadow.map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
                ef.shadow = Some(FontShadow {
                    offset: [x, y],
                    color,
                });
                ef.explicit.shadow = true;
            })
        })?,
    )?;
    m.set(
        "GetShadowOffset",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let o = read_font(lua, &this, elem)?
                .shadow
                .map_or([0.0, 0.0], |s| s.offset);
            Ok((o[0], o[1]))
        })?,
    )?;

    // ── 10/11 · spacing ─────────────────────────────────────────────────────────────────────
    // `SetSpacing([element,] n)` — `0x772240` clamps a negative argument to 0 before storing
    // (`0x772246 fcomp [0x7ffd74 = 0.0]`). This is the ONE way to open a gap between blocks: the
    // ctor default is 0, so out of the box they stack perfectly flush, with no per-element
    // constant, no half-line and no margin anywhere.
    m.set(
        "SetSpacing",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let n = f32::from_lua(arg(&rest, 0), lua)?.max(0.0);
            edit_font(lua, &this, elem, |ef| ef.spacing = n)
        })?,
    )?;
    m.set(
        "GetSpacing",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            Ok(read_font(lua, &this, elem)?.spacing)
        })?,
    )?;

    // ── 12..15 · justification ──────────────────────────────────────────────────────────────
    // Both resolve through the same `0x811ad0` table as `align`, masked `&7` / `&0x38`, so a
    // cross-axis token CLEARS its axis and a non-token raises (see [`crate::justify`]).
    //
    // **`SetJustifyH` is inert for rendered text**, and that is the byte law rather than a gap:
    // it writes the element font's justify, which `0x78adb0` overwrites on every block it builds
    // with the tag's own `align`. It is installed because the table has it and `GetJustifyH` must
    // answer what was stored.
    m.set(
        "SetJustifyH",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let s =
                String::from_lua(arg(&rest, 0), lua).map_err(|_| justify::usage_h("SimpleHTML"))?;
            let bits = justify::parse_bits(&s).ok_or_else(|| justify::usage_h("SimpleHTML"))?;
            edit_font(lua, &this, elem, |ef| {
                ef.justify.0 = justify::set_axis(ef.justify.0, justify::H_MASK, bits);
                ef.explicit.justify_h = true;
            })
        })?,
    )?;
    m.set(
        "GetJustifyH",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            Ok(read_font(lua, &this, elem)?.justify.name_h())
        })?,
    )?;
    m.set(
        "SetJustifyV",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            let s =
                String::from_lua(arg(&rest, 0), lua).map_err(|_| justify::usage_v("SimpleHTML"))?;
            let bits = justify::parse_bits(&s).ok_or_else(|| justify::usage_v("SimpleHTML"))?;
            edit_font(lua, &this, elem, |ef| {
                ef.justify.0 = justify::set_axis(ef.justify.0, justify::V_MASK, bits);
                ef.explicit.justify_v = true;
            })
        })?,
    )?;
    m.set(
        "GetJustifyV",
        lua.create_function(|lua, (this, mut rest): (Table, MultiValue)| {
            let elem = take_element(&mut rest);
            Ok(read_font(lua, &this, elem)?.justify.name_v())
        })?,
    )?;

    // ── 16..18 · the three with NO element argument ─────────────────────────────────────────
    // `SetText(s)` → 0 values (`0x796a90` discards the shared implementation's `usedMarkup`).
    // A nil argument reaches `lua_tostring` as NULL; an empty string is the closest thing to that
    // this engine can hand a parser, and it takes the same fallback route a NULL would.
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            let raw = match &text {
                // Bytes, lossily — never a raise (2138).
                Value::String(s) => s.to_string_lossy(),
                Value::Number(_) | Value::Integer(_) => super::object::as_f32(&text).to_string(),
                _ => String::new(),
            };
            let h = html_handle(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            set_text(&mut model, h, &raw);
            Ok(())
        })?,
    )?;
    // `SetHyperlinkFormat(s)` raises `Usage: %s:SetHyperlinkFormat("format")` (`0x87bb40`) if
    // argument 2 is not a string (`0x796bcc`–`0x796c29`). It takes effect on the NEXT parse.
    m.set(
        "SetHyperlinkFormat",
        lua.create_function(|lua, (this, fmt): (Table, Value)| {
            let Value::String(s) = &fmt else {
                return Err(mlua::Error::runtime(
                    "Usage: <SimpleHTML>:SetHyperlinkFormat(\"format\")",
                ));
            };
            let fmt = s.to_str()?.to_string();
            let h = html_handle(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.simple_html.entry(h).or_default().hyperlink_format = fmt;
            Ok(())
        })?,
    )?;
    m.set(
        "GetHyperlinkFormat",
        lua.create_function(|lua, this: Table| {
            let h = html_handle(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model
                .simple_html
                .get(&h)
                .map(|s| s.hyperlink_format.clone())
                .unwrap_or_else(|| parse::DEFAULT_HYPERLINK_FORMAT.to_string()))
        })?,
    )?;

    lua.set_named_registry_value(REG_SIMPLEHTML_METHODS, m)?;
    Ok(())
}

/// `(r, g, b [, a])` off the post-element argument list — alpha `lua_isnumber`-gated with the
/// default `1.0`, r/g/b required, exactly as the shared colour setters take them.
fn rgba(lua: &Lua, rest: &MultiValue) -> mlua::Result<[f32; 4]> {
    let r = f32::from_lua(arg(rest, 0), lua)?;
    let g = f32::from_lua(arg(rest, 1), lua)?;
    let b = f32::from_lua(arg(rest, 2), lua)?;
    let a = match arg(rest, 3) {
        v @ (Value::Number(_) | Value::Integer(_)) => f32::from_lua(v, lua)?,
        _ => 1.0,
    };
    Ok([r, g, b, a])
}

/// The per-frame state store — `Model`'s side of this module (see [`SimpleHtmlState`]).
pub(crate) type SimpleHtmlStates = HashMap<FrameHandle, SimpleHtmlState>;
