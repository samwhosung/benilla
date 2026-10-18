//! The `SimpleHTML` widget (`CSimpleHTML`, size `0x374`, ctor `0x789dd0`): the blocks [`parse`]
//! produces, built as regions, its four element fonts, and its Lua method table.
//!
//! `SetText` drops the previous blocks and builds the parsed ones. A text block is a FontString at
//! the frame's declared width with no height; block 0 hangs from the frame's TOPLEFT and block N
//! from block N-1's BOTTOMLEFT, `spacing` lower (0 by default).
//!
//! A block is LEFT unless its `align` says otherwise, never the FontString ctor's CENTER
//! (`0x78a7c8`, `0x78ae78`). The element fonts are font nodes that start empty; each block follows
//! its element's, or `P`'s while that one holds no face (`0x78ae30`), with no header scaling. There is no `GetContentHeight`: the hosting
//! ScrollFrame measures the blocks as regions (`0x786e30`).
//!
//! Deviation: the block step is not pixel-snapped (`0x766750`), because layout here is in logical
//! units; the two differ by under a pixel, and not at all at the default spacing of 0. The spacing
//! is also each block's line gap, as the block FontString inherits it. Deviation: a block of width
//! 0, in a frame sized by anchors, gets its text's natural width where the reference keeps 0,
//! because every width-less FontString here does.
//!
//! The method table (`0x87ba80`) has 19 entries, all installed; the first sixteen take an optional
//! leading element name (`0x795d80`). Build 5875 has no `GetText`.

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi;
use super::font::FontId;
use super::object::frame_handle_of;
use super::{Model, RegionData};
use crate::justify::{self, Justify};
use crate::layout::{Anchor, Anchors, Point};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, FrameKind, RegionHandle, RegionKind};

mod parse;

pub(crate) use parse::Block;
use parse::{ELEMENT_NAMES, ELEM_P};

/// Registry key of the SimpleHTML method table (the MAXCSTACK discipline).
pub(super) const REG_SIMPLEHTML_METHODS: &str = "__benilla_simplehtml_methods";

// ── State ────────────────────────────────────────────────────────────────────────────────────

/// A `SimpleHTML` frame's state, the members `CSimpleHTML` adds; in a [`Model`] map, not
/// [`crate::widget::KindState`], because it holds script-layer types.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SimpleHtmlState {
    /// The four element fonts, P, H1, H2 and H3 (`+0x350` to `+0x35c`, made empty by the ctor at
    /// `0x789e61`-`0x789ea3`): font nodes, which the blocks follow.
    pub(crate) fonts: [FontId; 4],
    /// `+0x360`, ctor `"|H%s|h%s|h"` (`0x87a838`).
    pub(crate) hyperlink_format: String,
    /// The content list (`+0x340`): every region the last `SetText` built, freed by the next.
    pub(crate) blocks: Vec<RegionHandle>,
    /// The link-button rebuild flag (`+0x370`, zeroed by the ctor at `0x789eb9`): set by `SetText`
    /// (`0x78a50d`) and by a new face or spacing on an element font (`0x78a380`), and spent by
    /// the frame's update (`0x78a30b`-`0x78a310`).
    pub(crate) links_dirty: bool,
}

/// SimpleHTML `fh`'s state, made with its four element fonts on first use, which is the
/// constructor's state.
pub(crate) fn state_mut(model: &mut Model, fh: FrameHandle) -> &mut SimpleHtmlState {
    let Model {
        simple_html, fonts, ..
    } = model;
    simple_html.entry(fh).or_insert_with(|| SimpleHtmlState {
        fonts: std::array::from_fn(|_| fonts.element_of(fh)),
        hyperlink_format: parse::DEFAULT_HYPERLINK_FORMAT.to_string(),
        blocks: Vec::new(),
        links_dirty: false,
    })
}

// ── SetText, the rebuild ─────────────────────────────────────────────────────────────────────

/// `CSimpleHTML::SetText` (`0x78a3a0`): free the old blocks, parse, build. Returns `usedMarkup`
/// (`0x78a519`), which the Lua shim drops.
pub(crate) fn set_text(model: &mut Model, fh: FrameHandle, raw: &str) -> bool {
    // Free the previous parse's regions first, as the reference does (`0x78a580`), or a book's
    // next page draws over the last.
    let old = model
        .simple_html
        .get_mut(&fh)
        .map(|s| std::mem::take(&mut s.blocks))
        .unwrap_or_default();
    crate::script::region::free_regions(model, &old);

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
    state_mut(model, fh).links_dirty = true;
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
                // A block follows its element's font, or `P`'s while that one holds no face
                // (`0x78ae29`-`0x78ae54`), so a lone `<FontString>` serves every element.
                let font = {
                    let fonts = state_mut(model, fh).fonts;
                    let own = fonts[*elem];
                    if model.fonts.get(own).block.font.is_some() {
                        own
                    } else {
                        fonts[ELEM_P]
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
                // The block's text goes through `SetText` with its expand flag (`0x78aea1`).
                let text = model.localized_text(text.clone());
                model.region_data.insert(
                    rh,
                    RegionData {
                        anchors: Anchors::from([anchor_for(
                            prev_block,
                            frame_id,
                            Point::TopLeft,
                            next_y,
                        )]),
                        // No height: the rect takes the wrapped text's height from the measure
                        // round trip, as the client's font engine gives it (`0x7729b0`,
                        // `0x5c2070`).
                        size: Some((width, 0.0)),
                        text: Some(text),
                        ..RegionData::default()
                    },
                );
                // The link applies every group the font holds; a later set on the element font
                // pushes to the blocks already built.
                super::font::link_region(model, rh, Some(font));
                let d = model.region_data.get_mut(&rh).expect("inserted above");
                // The tag's `align` is written after the link (`0x78ae78`), so it beats the
                // font's justifyH; justifyV stays the font's, inert in a rect its text fills.
                // Neither axis is inherited again: `0x78ae68` clears the horizontal bits of the
                // per-bit mask and `0x78ae95` the group bit, so a later push never re-justifies
                // a built block.
                d.justify_mask.0 &= !justify::H_MASK;
                d.justify = Justify(justify::set_axis(d.justify.0, justify::H_MASK, *align));
                d.font_mask.clear(super::FontMask::JUSTIFY);
                // The block FontString's field, which its setter clamped (`0x78aeaf`).
                next_y = -d.spacing.max(0.0);
                model.touch_measure(rh); // a text block arrives with its text
                prev_block = Some(id);
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
                // Minted though nothing anchors to an image; the resolve and the free use it.
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
                    .map(|p| Anchors::from([anchor_for(prev_block, frame_id, p, next_y)]))
                    .unwrap_or_default();
                model.region_data.insert(
                    rh,
                    RegionData {
                        anchors,
                        size: Some((*w, *h)),
                        ..RegionData::default()
                    },
                );
                if let Some(src) = src.clone() {
                    model.set_texel(rh, crate::script::texel::TexSet::File(src));
                }
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

    // Each new block's `SetPoint` invalidates it (`0x78ae0c`, flag 1); the old ones' frees named
    // theirs.
    for &rh in &made {
        model.invalidate_region(rh, true);
    }
    state_mut(model, fh).blocks = made;
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

// ── The loader's seam ────────────────────────────────────────────────────────────────────────

/// SimpleHTML `frame`'s four element fonts, for the loader: its `font=` links all four
/// (`0x78a17a`-`0x78a18f`) and each font child loads into one (`0x78a26e`).
pub(crate) fn element_fonts_lua(lua: &Lua, frame: &Table) -> mlua::Result<[FontId; 4]> {
    let h = html_handle(lua, frame)?;
    Ok(state_mut(&mut lua.app_data_mut::<Model>().expect("model"), h).fonts)
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

/// The font the first sixteen methods act on: the element the optional leading name selects
/// ([`take_element`]), which is removed before the shared implementation reads the rest.
fn element_font(lua: &Lua, this: &Table, args: &mut MultiValue) -> mlua::Result<FontId> {
    let h = html_handle(lua, this)?;
    let elem = take_element(args);
    Ok(state_mut(&mut lua.app_data_mut::<Model>().expect("model"), h).fonts[elem])
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // ── 0..15 · the element fonts, through the Font's own implementations ──────────────────
    // `SetJustifyH` never shows: `0x78adb0` overwrites it with each block's `align`.
    super::font::install_shared(lua, &m, element_font)?;

    // ── 16..18 · the three without an element argument ──────────────────────────────────────
    // SetText(s) returns nothing (`0x796a90` drops `usedMarkup`); nil parses as an empty string,
    // which takes the fallback a NULL takes.
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            // `lua_tostring(2)` (`0x796b1e`): bytes, lossily, a number as Lua's `%.14g` text.
            let raw = binding_abi::optional_string(lua, &text).unwrap_or_default();
            let h = html_handle(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            set_text(&mut model, h, &raw);
            Ok(())
        })?,
    )?;
    // SetHyperlinkFormat(s) takes any `lua_isstring` value (`0x796bcc`), a number as its `%.14g`
    // text, and raises `Usage: %s:SetHyperlinkFormat("format")` (`0x87bb40`) naming the frame
    // otherwise (`0x796bef`-`0x796c21`); it applies from the next parse.
    m.set(
        "SetHyperlinkFormat",
        lua.create_function(|lua, (this, fmt): (Table, Value)| {
            let h = html_handle(lua, &this)?;
            let Some(fmt) = binding_abi::optional_string(lua, &fmt) else {
                return Err(binding_abi::usage(
                    lua,
                    &this,
                    "SetHyperlinkFormat(\"format\")",
                ));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            state_mut(&mut model, h).hyperlink_format = fmt;
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

/// The per-frame [`SimpleHtmlState`] store `Model` holds.
pub(crate) type SimpleHtmlStates = HashMap<FrameHandle, SimpleHtmlState>;
