//! The chat bubble widgets: the reference's `CGChatBubbleFrame` (ctor `0x4b0940`), an anonymous
//! `Frame` the pool `0x4b1870` builds with the `WorldFrame` as its parent (the spawn passes
//! `[0xb4b2bc]`, `0x608bb9`), so it takes the `WorldFrame`'s stratum (`SetParent 0x76ab10`).
//! Bubble addons (pfUI's `bubbles.lua`) find it in `WorldFrame:GetChildren()` by its first
//! region's texture and read it positionally: eleven regions in creation order, the backdrop's
//! background and eight border pieces (`0x4b0a35` → `0x77e8d0`), the tail (`0x4b0a57`) and the
//! text (`0x4b0b14`).
//!
//! The app owns each line's lifetime, fade and seat ([`BubbleState`]); this layer owns the frames:
//! it hands one out per line, takes it back when the line ends, and writes what moved.

use std::collections::{HashMap, VecDeque};

use crate::layout::{Anchor, Anchors, Point};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, FrameKind, RegionHandle, RegionKind};

use super::backdrop::{self, Backdrop, Insets};
use super::model::Model;
use super::UiScript;

/// Region 1's art, the backdrop's background, and the path bubble addons identify a bubble by.
pub const BG_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Background";
/// Regions 2 to 9, the backdrop's eight border pieces.
pub const EDGE_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Backdrop";
/// Region 10, the tail.
pub const TAIL_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Tail";

/// The first frame level the per-frame pass stamps, farthest bubble first (`0x4b12de`).
const FIRST_LEVEL: u16 = 2;

/// The bubble geometry that follows from the window rather than the line, in the `WorldFrame`'s
/// units. Written into a bubble when it changes, so an addon's own anchors survive between
/// resizes. A resize re-sizes the live bubbles as part of the live re-layout's deviation, where
/// the reference can reload the UI on an aspect change (`0x492f13` → `0x491380`); the `Deviation:`
/// line is at the resize in `benilla_app`'s `extract::tick_script`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BubbleGeometry {
    /// The border unit (`0x4b09d2`-`0x4b0a2f`): the backdrop's edge size, tile size and four
    /// insets, the tail's side, and half the text's width floor.
    pub border: f32,
    /// The frame's margin around the text on all four sides (`0x4b0b6b`, `0x4b0b94`).
    pub margin: f32,
    /// The text's width cap, past which it wraps (`0x4b175d`).
    pub wrap_cap: f32,
    /// The text's font height (`0x4b0b21`).
    pub text_height: f32,
    /// Device pixels per unit, the grid the seat snaps to.
    pub device_px: f32,
}

/// One live bubble's state for this frame, in the active list's order: farthest from the camera
/// first, which is the frame level order.
#[derive(Clone, Debug, PartialEq)]
pub struct BubbleState {
    /// The speaker's GUID: a speaker has at most one bubble (`[unit+0xe64]`).
    pub key: u64,
    /// The chat line, unique per spawn: a new value for a live key is a new line, which replaces
    /// the bubble (`0x608bb4`, `0x608bbf`).
    pub line: u64,
    /// The display text, already through the bubble's escape strip.
    pub text: String,
    /// The chat type's colour (`0x49e990`).
    pub colour: [f32; 3],
    /// The own-shown bit: a line is shown when it spawns and while it fades, hidden when an
    /// out-of-range fade ends (`0x4b0d9f`), shown again on the way back (`0x4b0ea0`).
    pub shown: bool,
    /// The fade's frame alpha (`0x4b0d4f`, `0x4b0dc3`).
    pub alpha: f32,
    /// The projected speaker point the text's BOTTOM sits on, from the `WorldFrame`'s
    /// BOTTOMLEFT (`0x4b157a`).
    pub seat: (f32, f32),
}

/// One pooled bubble's widgets, in the reference's creation order after the backdrop's nine.
#[derive(Debug)]
struct Bubble {
    frame: FrameHandle,
    tail: RegionHandle,
    text: RegionHandle,
    /// The `(key, line)` it shows; `None` in the free list.
    line: Option<(u64, u64)>,
    /// The own-shown bit as this layer last set it (`[frame+0xd0]`); a new frame is born shown by
    /// `CSimpleFrame`'s ctor (`0x76934a`).
    shown: bool,
    /// The anchor offset last written, the change gate's cache (`[bubble+0x324]`, `+0x328`).
    seat: Option<(f32, f32)>,
    /// The geometry its sizes were written under.
    laid_out: Option<BubbleGeometry>,
}

/// The bubble pool: grown on demand, never shrunk or reordered, so every bubble keeps its place in
/// the `WorldFrame`'s child list for the session.
#[derive(Debug, Default)]
pub(crate) struct ChatBubbles {
    bubbles: Vec<Bubble>,
    /// The free list (`[0xb6f064]`): a finished bubble joins the tail (`0x4b15e5`-`0x4b15f7`) and a
    /// new line takes the head (`0x4b1870`), so the bubble retired first is reused first.
    free: VecDeque<usize>,
    /// Each live speaker's pool slot, by [`BubbleState::key`].
    assigned: HashMap<u64, usize>,
    /// Frame to pool slot, never removed.
    by_frame: HashMap<FrameHandle, usize>,
    /// The active list (`[0xb6f038]`) as the last sync left it: the live slots, farthest first.
    active: Vec<usize>,
}

impl UiScript {
    /// Drive the pool from the app's live bubbles: end the lines that are gone, hand a frame to
    /// each new one, then write each live bubble's level, alpha, visibility and seat.
    pub fn sync_chat_bubbles(&mut self, geometry: BubbleGeometry, states: &[BubbleState]) {
        let lua = self.lua();
        // The font a new bubble takes: `FrameScript_GetText` on `NAMEPLATE_FONT` (`0x4b0b36` →
        // `0x703bf0`), whose `_G[key]` arm (`0x703c12`) answers the global's string, else the
        // shared "" a missing or non-string global gives, which sets no font.
        let font = || match lua.globals().get::<mlua::Value>("NAMEPLATE_FONT") {
            Ok(mlua::Value::String(s)) => Some(s.to_string_lossy()).filter(|s| !s.is_empty()),
            _ => None,
        };
        let visibility = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // No WorldFrame (no interface loaded), no bubbles: no addon walks an invented root.
            let Some(world) = model.arena.lookup("WorldFrame") else {
                return;
            };
            sync(&mut model, world, geometry, states, &font)
        };
        // Each a full transition with its handlers, in the order the sync issued them.
        for (frame, shown) in visibility {
            super::visibility::set_shown(lua, frame, shown);
        }
    }

    /// How many bubble widgets the pool holds.
    pub fn chat_bubble_pool_len(&self) -> usize {
        let lua = self.lua();
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.chat_bubbles.bubbles.len()
    }
}

/// Whether `frame` is a pooled bubble.
pub(super) fn is_bubble(model: &Model, frame: FrameHandle) -> bool {
    model.chat_bubbles.by_frame.contains_key(&frame)
}

/// The per-frame drive; returns the `Show`/`Hide` calls to run, in order.
fn sync(
    model: &mut Model,
    world: FrameHandle,
    geometry: BubbleGeometry,
    states: &[BubbleState],
    font: &dyn Fn() -> Option<String>,
) -> Vec<(FrameHandle, bool)> {
    let mut visibility = Vec::new();

    // Lines that ended, in the active list's order, farthest first, as the reference's walk
    // (`0x4b1138`) meets and destroys them (`0x4b1186`, `0x4b11c0`), so they join the free list in
    // that order, before any new line this frame looks for a frame.
    let live = |key: u64, line: u64| states.iter().any(|s| s.key == key && s.line == line);
    for i in std::mem::take(&mut model.chat_bubbles.active) {
        if let Some((key, line)) = model.chat_bubbles.bubbles[i].line {
            if !live(key, line) {
                retire(model, i, &mut visibility);
            }
        }
    }

    let world_id = model.frame_id(world);
    for (rank, state) in states.iter().enumerate() {
        let slot = match model.chat_bubbles.assigned.get(&state.key) {
            Some(&i) => i,
            None => {
                // A new line: the spawn's acquire (`0x4b1870`), then its text (`0x4b1600`).
                let i = acquire(model, world, geometry, font);
                model.chat_bubbles.assigned.insert(state.key, i);
                if model.chat_bubbles.bubbles[i].laid_out != Some(geometry) {
                    lay_out(model, i, geometry);
                }
                set_line(model, i, state, geometry, &mut visibility);
                i
            }
        };
        if model.chat_bubbles.bubbles[slot].laid_out != Some(geometry) {
            lay_out(model, slot, geometry);
            fit_width(model, slot, geometry);
        }
        drive(
            model,
            slot,
            world_id,
            rank,
            state,
            geometry,
            &mut visibility,
        );
        model.chat_bubbles.active.push(slot);
    }
    visibility
}

/// Take a frame for a new line: the free list's head, else a new one at the pool's tail.
fn acquire(
    model: &mut Model,
    world: FrameHandle,
    geometry: BubbleGeometry,
    font: &dyn Fn() -> Option<String>,
) -> usize {
    if let Some(i) = model.chat_bubbles.free.pop_front() {
        return i;
    }
    let bubble = create(model, world, geometry, font());
    let i = model.chat_bubbles.bubbles.len();
    model.chat_bubbles.by_frame.insert(bubble.frame, i);
    model.chat_bubbles.bubbles.push(bubble);
    i
}

/// End a bubble's line (`0x4b1590`): clear its shown bit and hide it, then put it on the free
/// list's tail. It keeps its parent and its place in the child list.
fn retire(model: &mut Model, i: usize, visibility: &mut Vec<(FrameHandle, bool)>) {
    let b = &mut model.chat_bubbles.bubbles[i];
    let Some((key, _)) = b.line.take() else {
        return;
    };
    if b.shown {
        b.shown = false;
        visibility.push((b.frame, false));
    }
    model.chat_bubbles.assigned.remove(&key);
    model.chat_bubbles.free.push_back(i);
}

/// Build one bubble (`0x4b0940`): the Frame under the `WorldFrame`, the backdrop's nine regions,
/// the tail, the text, and the frame's two anchors on the text.
fn create(
    model: &mut Model,
    world: FrameHandle,
    g: BubbleGeometry,
    font: Option<String>,
) -> Bubble {
    // Anonymous: the pool passes no name.
    let frame = model.arena.create(FrameKind::Frame, None, Some(world));
    // `SetParent 0x76ab10`: the parent's stratum and its level plus one, until the first level
    // stamp.
    let (strata, level) = model
        .arena
        .frame(world)
        .map(|f| (f.strata, f.level))
        .unwrap_or_default();
    model.arena.set_frame_strata(frame, strata);
    model
        .arena
        .set_frame_level(frame, level.saturating_add(1), false);

    // The backdrop first (`0x4b0a35` → `0x76a5d0`): edge mask 0xff and tile on (`0x4b09bf`,
    // `0x4b09c6`), one border unit for the four insets (`0x4b09ec`-`0x4b09f5`), the edge size
    // (`0x4b0a12`, `+0x48`) and the tile size (`0x4b0a2f`, `+0x4c`).
    backdrop::install(
        model,
        frame,
        Some(Backdrop {
            bg_file: Some(BG_TEXTURE.to_string()),
            edge_file: Some(EDGE_TEXTURE.to_string()),
            tile: true,
            tile_size: g.border,
            edge_size: g.border,
            insets: uniform(g.border),
            ..Backdrop::default()
        }),
    );
    // Then the tail on ARTWORK (`0x4b0a57`, layer 2) and the text on ARTWORK (`0x4b0b14`, layer 2).
    let tail = model
        .arena
        .create_region(frame, RegionKind::Texture, DrawLayer::Artwork, 0)
        .expect("live bubble");
    model.set_texel(tail, super::texel::TexSet::File(TAIL_TEXTURE.to_string()));
    let text = model
        .arena
        .create_region(frame, RegionKind::FontString, DrawLayer::Artwork, 0)
        .expect("live bubble");
    // The `NAMEPLATE_FONT` global's font with no flags (`0x4b0b2b`-`0x4b0b42`): no outline, no
    // shadow.
    let data = model.region_data.entry(text).or_default();
    data.font_path = font;
    data.font_explicit.face = true;
    model.touch_layout_frame(frame);
    model.touch_layout_region(tail);
    model.touch_layout_region(text);

    Bubble {
        frame,
        tail,
        text,
        line: None,
        shown: true,
        seat: None,
        laid_out: None,
    }
}

fn uniform(v: f32) -> Insets {
    Insets {
        left: v,
        right: v,
        top: v,
        bottom: v,
    }
}

/// Write the window-derived sizes and anchors: the backdrop's unit, the tail (`0x4b0a7d`-
/// `0x4b0af1`), the font height and the frame's margin anchors on the text.
fn lay_out(model: &mut Model, i: usize, g: BubbleGeometry) {
    let b = &model.chat_bubbles.bubbles[i];
    let (frame, tail, text) = (b.frame, b.tail, b.text);
    backdrop::set_metrics(model, frame, g.border, g.border, uniform(g.border));
    let frame_id = model.frame_id(frame);
    let text_id = model.region_id(text);

    // A border-unit square, its TOPRIGHT on the frame's BOTTOM lifted a quarter unit.
    let tail_data = model.region_data.entry(tail).or_default();
    tail_data.size = Some((g.border, g.border));
    tail_data.anchors = Anchors::from([Anchor::new(
        Point::TopRight,
        frame_id,
        Point::Bottom,
        0.0,
        g.border / 4.0,
    )]);

    model.region_data.entry(text).or_default().font_height = Some(g.text_height);
    model.touch_measure(text);

    // The frame hugs the text: TOPLEFT and BOTTOMRIGHT on the text's, out by the margin.
    let input = model.layout_inputs.entry(frame).or_default();
    input.anchors = Anchors::from([
        Anchor::new(Point::TopLeft, text_id, Point::TopLeft, -g.margin, g.margin),
        Anchor::new(
            Point::BottomRight,
            text_id,
            Point::BottomRight,
            g.margin,
            -g.margin,
        ),
    ]);
    // Once per geometry change: each node it wrote is invalidated, as the bubble build's
    // `SetPoint`s with the invalidate flag do (`0x4b0b6b`, `0x4b0b94`).
    model.invalidate_frame(frame, true);
    model.invalidate_region(tail, true);
    model.invalidate_region(text, true);
    model.chat_bubbles.bubbles[i].laid_out = Some(g);
}

/// Show a new line (`0x4b1600`): its text and colour, its width fitted, then `Show` through the
/// fade-in's start (`0x4b17f7` → `0x4b0ea0` → vtable `+0x88`), which fires `OnShow` on a reused,
/// hidden frame.
fn set_line(
    model: &mut Model,
    i: usize,
    state: &BubbleState,
    g: BubbleGeometry,
    visibility: &mut Vec<(FrameHandle, bool)>,
) {
    let b = &mut model.chat_bubbles.bubbles[i];
    b.line = Some((state.key, state.line));
    let (frame, text, was_shown) = (b.frame, b.text, b.shown);
    b.shown = true;
    let c = state.colour;
    // The line goes through `SetText` with its expand flag (`0x4b16c0`).
    let line = model.localized_text(state.text.clone());
    let data = model.region_data.entry(text).or_default();
    data.text = Some(line);
    data.set_vertex_color(Some([c[0], c[1], c[2], 1.0]));
    fit_width(model, i, g);
    if !was_shown {
        visibility.push((frame, true));
    }
}

/// The width law (`0x4b16c5`-`0x4b1796`): with width and height cleared, the text's own width;
/// past the cap the cap, so it wraps, else at least two border units. The height stays cleared,
/// so it is the wrapped text's.
fn fit_width(model: &mut Model, i: usize, g: BubbleGeometry) {
    let text = model.chat_bubbles.bubbles[i].text;
    if let Some(d) = model.region_data.get_mut(&text) {
        d.size = None;
    }
    model.touch_measure(text);
    super::measure::ensure_measured_in(model, text);
    let natural = super::region::virtual_span(model, text).0;
    let floor = 2.0 * g.border;
    let width = if natural > g.wrap_cap {
        g.wrap_cap
    } else {
        natural.max(floor)
    };
    if let Some(d) = model.region_data.get_mut(&text) {
        d.size = Some((width, 0.0));
    }
    model.touch_measure(text);
    // `SetWidth`, which invalidates (`0x7683d0`).
    model.invalidate_region(text, true);
}

/// The per-frame write, each property only when it moved.
fn drive(
    model: &mut Model,
    i: usize,
    world_id: u32,
    rank: usize,
    state: &BubbleState,
    g: BubbleGeometry,
    visibility: &mut Vec<(FrameHandle, bool)>,
) {
    let b = &model.chat_bubbles.bubbles[i];
    let (frame, text, shown, last_seat) = (b.frame, b.text, b.shown, b.seat);

    // `0x76a4f0(2 + i, 0)` down the list: no carry to children (`0x4b1306`).
    let level = u16::try_from(rank)
        .unwrap_or(u16::MAX)
        .saturating_add(FIRST_LEVEL);
    model.arena.set_frame_level(frame, level, false);

    if model
        .arena
        .frame(frame)
        .is_some_and(|f| f.alpha != state.alpha)
    {
        model.arena.set_alpha(frame, state.alpha);
    }
    if shown != state.shown {
        model.chat_bubbles.bubbles[i].shown = state.shown;
        visibility.push((frame, state.shown));
    }

    // The seat (`0x4b14a0`): the text's BOTTOM on the `WorldFrame`'s BOTTOMLEFT, re-anchored only
    // when the snapped point moved. Deviation: the frame's bottom-left origin is snapped onto the
    // device grid so the border art blits 1:1; the reference ceils the anchor point itself onto
    // it (`0x7666a0`, `0x766750`).
    let width = model
        .region_data
        .get(&text)
        .and_then(|d| d.size)
        .map_or(0.0, |s| s.0)
        + 2.0 * g.margin;
    let snap = |v: f32| {
        if g.device_px > 0.0 {
            (v * g.device_px).round() / g.device_px
        } else {
            v
        }
    };
    let seat = (
        snap(state.seat.0 - width * 0.5) + width * 0.5,
        snap(state.seat.1 - g.margin) + g.margin,
    );
    if last_seat != Some(seat) {
        let data = model.region_data.entry(text).or_default();
        data.anchors = Anchors::from([Anchor::new(
            Point::Bottom,
            world_id,
            Point::BottomLeft,
            seat.0,
            seat.1,
        )]);
        // `SetPoint` with the invalidate flag (`0x4b157a`).
        model.invalidate_region(text, true);
        model.chat_bubbles.bubbles[i].seat = Some(seat);
    }
}
