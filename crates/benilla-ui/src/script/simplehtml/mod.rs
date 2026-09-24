//! The `SimpleHTML` widget (`CSimpleHTML`, size `0x374`, ctor `0x789dd0`): the blocks [`parse`]
//! produces, built as regions, its four element fonts, and its Lua method table.
//!
//! `SetText` drops the previous blocks and builds the parsed ones. A text block is a FontString at
//! the frame's declared width with no height; block 0 hangs from the frame's TOPLEFT and block N
//! from block N-1's BOTTOMLEFT, `spacing` lower (0 by default).
//!
//! A block is LEFT unless its `align` says otherwise, never the FontString ctor's CENTER
//! (`0x78a7c8`, `0x78ae78`). An element font starts empty, and `H1` to `H3` use `P`'s while their
//! path is empty (`0x78ae30`), with no header scaling. There is no `GetContentHeight`: the hosting
//! ScrollFrame measures the blocks as regions (`0x786e30`).
//!
//! Deviation: the block step is not pixel-snapped (`0x766750`), because layout here is in logical
//! units; the two differ by under a pixel, and not at all at the default spacing of 0. Line spacing
//! inside a block is not applied, as nothing here models line spacing; `SetSpacing` sets the step
//! between blocks. Deviation: a block of width 0, in a frame sized by anchors, gets its text's
//! natural width where the reference keeps 0, because every width-less FontString here does.
//!
//! The method table (`0x87ba80`) has 19 entries, all installed; the first sixteen take an optional
//! leading element name (`0x795d80`). Build 5875 has no `GetText`.

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

/// Registry key of the SimpleHTML method table (the MAXCSTACK discipline).
pub(super) const REG_SIMPLEHTML_METHODS: &str = "__benilla_simplehtml_methods";

// ── State ────────────────────────────────────────────────────────────────────────────────────

/// One of the four element fonts (`CSimpleHTML+0x350`, P, H1, H2, H3; made at
/// `0x789e61`-`0x789ea3`): the font object it inherits, a live link as the reference's
/// `parentFontObject` is, plus what it set itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ElementFont {
    /// `inherits=` or `SetFontObject`: the object this element resolves through.
    pub(crate) font_object: Option<String>,
    /// What this element set itself, the region's severance mask (`FONTINSTANCE+0x038`), which a
    /// block inherits.
    pub(crate) explicit: super::FontExplicit,
    /// `+0x3c`, empty at the ctor: the path `0x78ae30` tests for the fallback to `P`.
    pub(crate) font_path: Option<String>,
    /// `FieldBlock` height.
    pub(crate) font_height: Option<f32>,
    /// `SetFont`'s flags / XML `outline=`.
    pub(crate) outline: Outline,
    /// `SetTextColor` / `<Color>`.
    pub(crate) color: Option<[f32; 4]>,
    /// `SetShadowColor`/`SetShadowOffset` / `<Shadow>`.
    pub(crate) shadow: Option<FontShadow>,
    /// `CSimpleFont+0x54`, ctor `0x212` (`0x783a98`): CENTER, MIDDLE and the one-to-one bit.
    pub(crate) justify: Justify,
    /// `+0x50`, ctor 0 (`0x783a81`): the step between blocks and, in the reference, the line gap.
    pub(crate) spacing: f32,
}

/// A `SimpleHTML` frame's state, the members `CSimpleHTML` adds; in a [`Model`] map, not
/// [`crate::widget::KindState`], because it holds script-layer types.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SimpleHtmlState {
    /// `+0x350` to `+0x35c`.
    pub(crate) fonts: [ElementFont; 4],
    /// `+0x360`, ctor `"|H%s|h%s|h"` (`0x87a838`).
    pub(crate) hyperlink_format: String,
    /// The content list (`+0x340`): every region the last `SetText` built, freed by the next.
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

/// The paint a block copies from its element font: `SetFontObject` (`0x770c60`) pulls all five
/// property groups at once (`0x770d06`), with the element's own sets on top.
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

// ── SetText, the rebuild ─────────────────────────────────────────────────────────────────────

/// `CSimpleHTML::SetText` (`0x78a3a0`): free the old blocks, parse, build. Returns `usedMarkup`
/// (`0x78a519`), which the Lua shim drops.
pub(crate) fn set_text(model: &mut Model, fh: FrameHandle, raw: &str) -> bool {
    // Free the previous parse's regions first, as the reference does, or a book's next page
    // draws over the last.
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
    // Deviation: the reference prints these to the console; here they are host warnings, because
    // a malformed page is bad content, not a script error.
    for e in &parsed.errors {
        model.record_warning(e.clone());
    }

    build(model, fh, &parsed.blocks);
    parsed.used_markup
}

/// The block loop of `AddTextBlock` (`0x78adb0`) and the `<IMG>` handler (`0x78ab40`): anchors,
/// fonts and the running `nextY`.
fn build(model: &mut Model, fh: FrameHandle, blocks: &[Block]) {
    let frame_id = model.frame_id(fh);
    // The frame's declared width, not its resolved rect (`0x78ae19`, `0x768420`); a later resize
    // does not re-wrap (`OnRectChanged` `0x78a320`), as each block keeps the width it was built at.
    let width = model
        .layout_inputs
        .get(&fh)
        .map_or(0.0, |input| input.width);

    // `prevBlock` (`+0x348`), the last text block, and `nextYOffset` (`+0x34c`), never positive.
    let mut prev_block: Option<u32> = None;
    let mut next_y = 0.0f32;
    let mut made: Vec<RegionHandle> = Vec::with_capacity(blocks.len());

    for block in blocks {
        match block {
            Block::Text { text, elem, align } => {
                // An element with an empty path uses `P`'s font (`0x78ae29`-`0x78ae54`), so a
                // lone `<FontString>` serves every element.
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
                    // Unreachable; stop, keeping what was built recorded for the next `SetText`.
                    break;
                };
                let id = model.region_id(rh);
                let mut d = RegionData {
                    anchors: vec![anchor_for(prev_block, frame_id, Point::TopLeft, next_y)],
                    // No height: the rect takes the wrapped text's height from the measure round
                    // trip, as the client's font engine gives it (`0x7729b0`, `0x5c2070`).
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
                // The tag's `align` is written after `SetFontObject` (`0x78ae78`), so it beats
                // the font's justifyH; justifyV stays the font's, inert in a rect its text fills.
                d.justify = Justify(justify::set_axis(paint.justify.0, justify::H_MASK, *align));
                // Neither axis is inherited again (`0x78ae68`, `0x78ae95` clear the inherit bits),
                // so a later font-object change never re-justifies a built block.
                d.font_explicit.justify_h = true;
                d.font_explicit.justify_v = true;
                model.region_data.insert(rh, d);
                model.touch_measure(rh); // a text block arrives with its text
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
                    break; // unreachable, as above
                };
                // Minted though nothing anchors to an image; the resolve and `free_block` use it.
                model.region_id(rh);
                // `align` picks the corner; 8, 16 and 32 get no anchor at all (`0x78ac61`, a jump
                // to `0x78ace8`), so the image draws nothing, here as there.
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
                // An unfloated image moves the flow down by its height (`0x78ad07`-`0x78ad1b`), but
                // the next block still hangs off the last text block: the image path never writes
                // `prevBlock`. Without `height=` it reserves its texture's texel height
                // (`CSimpleTexture`'s `GetHeight`), from the probe the resolve sizes it by.
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

/// A block's anchor (`SetPoint` `0x767c70`): block 0 to the frame, block N to the previous text
/// block's bottom edge, `nextYOffset` lower.
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

fn free_block(model: &mut Model, rh: RegionHandle) {
    crate::script::region::free_region(model, rh);
}

// ── The loader's seam ────────────────────────────────────────────────────────────────────────

/// Apply the `font=`, `<FontHeight>` and `outline=` an XML `<FontString>` or `<FontStringHeaderN>`
/// child supplies, any of them absent. The reference applies these in C++ (`CSimpleFont::LoadXML`
/// `0x783c30`, from `0x78a1fe`), not through `SetFont`, which requires both path and height.
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

/// The element index an XML child tag names: `<FontString>` is `P`, `<FontStringHeader1|2|3>`
/// `H1`, `H2`, `H3` (names `0x8786cc`, `0x87a8b4`, `0x87a8a0`, `0x87a88c`; read at `0x78a1fe` to
/// `0x78a26e`).
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

// ── The Lua surface ──────────────────────────────────────────────────────────────────────────

/// Resolve `this` to a live `SimpleHTML` frame; only a misapplied method table fails.
fn html_handle(lua: &Lua, this: &Table) -> mlua::Result<FrameHandle> {
    let h = frame_handle_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model");
    match model.arena.frame(h).map(|f| f.kind) {
        Some(FrameKind::SimpleHtml) => Ok(h),
        _ => Err(mlua::Error::runtime("not a SimpleHTML")),
    }
}

/// The optional leading element name of the first sixteen methods (`0x795d80`). `P`, `H1`, `H2`
/// or `H3`, in any case, is removed and selects that element; anything else selects `P` and stays
/// (`0x795e50`), so `SetFont("h4", path, 15)` sets `P`'s font with `"h4"` as the path.
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

/// Run `f` over one element font under a short borrow; it shapes the next `SetText`'s blocks.
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

/// One element font's resolved paint: what the getters answer and a block would be built with.
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

    // ── 0/1 · the font object ────────────────────────────────────────────────────────────────
    // SetFontObject([element,] font | "font" | nil) returns nothing and changes only the next
    // `SetText`'s blocks; a built block is never re-fonted (`0x795d3e`).
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
            // nil severs the link and keeps the paint, as the reference stores a null parent; an
            // element font resolves its object lazily, so the standing paint is pinned here.
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
                // A re-point keeps the severance mask, as on a region: each local setter clears
                // its bit of the inherit mask (`FONTINSTANCE+0x2c`) for good.
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
    // SetFont([element,] file, height [, flags]) returns 1, or nil on an empty path; the shared
    // gate (`0x79f210`) takes numeric strings and raises `0x87c69c`'s usage line on anything else.
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
                    // The Lua flags spelling, not XML's `outline=` ([`Outline::flags`]).
                    ef.outline = Outline::flags(&f);
                    ef.explicit.outline = true;
                }
            })?;
            Ok(if ok { Value::Number(1.0) } else { Value::Nil })
        })?,
    )?;
    // GetFont([element]): path, height and a flags string, `""` rather than nil.
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
    // `GetShadowColor` returns four values (`0x79f9b3`), `GetShadowOffset` two; each setter keeps
    // the other half.
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
    // Both arguments are required: the shared code raises its usage line (`0x87c6e8`).
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
    // SetSpacing([element,] n) clamps a negative to 0 (`0x772240`, `0x772246`); it is the only gap
    // between blocks, which stack flush at the ctor default of 0.
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
    // Both parse through `align`'s table (`0x811ad0`), masked `&7` or `&0x38` ([`crate::justify`]).
    // `SetJustifyH` never shows: `0x78adb0` overwrites it with each block's `align`.
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

    // ── 16..18 · the three without an element argument ──────────────────────────────────────
    // SetText(s) returns nothing (`0x796a90` drops `usedMarkup`); nil parses as an empty string,
    // which takes the fallback a NULL takes.
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            let raw = match &text {
                // Bytes, lossily; never a raise.
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
    // SetHyperlinkFormat(s) raises its usage line (`0x87bb40`) on a non-string
    // (`0x796bcc`-`0x796c29`); it applies from the next parse.
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

/// `(r, g, b [, a])` after the element argument, as the shared colour setters take it: r, g and b
/// required, alpha 1 unless a number.
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

/// The per-frame [`SimpleHtmlState`] store `Model` holds.
pub(crate) type SimpleHtmlStates = HashMap<FrameHandle, SimpleHtmlState>;
