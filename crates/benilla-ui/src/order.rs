//! Draw order: the strata and draw-layer vocabulary, the packed total-order key [`ZKey`], and the
//! visible-tree [`traversal`] in the client's painter order.
//!
//! The 1.12 client draws in one flat painter's order, not a tree walk: every visible frame, child
//! or not, is an entry in a per-`(strata, level)` bucket (`root+0xcd4`), so a child with a lower
//! strata or level draws before its parent. Inside a bucket the draw layer outranks the frame: the
//! five layer batches belong to the level node (`levelNode+0x1c`), and the emitter `0x765920` loops
//! layers outside and frames inside, so every frame's BACKGROUND draws before any frame's BORDER.
//! Within a layer, `0x76fb00` drains every quad, then every string, then the render callbacks.
//!
//! A scroll child's subtree is the one tree walk. `SetScrollChild` flags it out of the flat pass
//! (`0x7659df`), and its ScrollFrame registers a HIGHLIGHT callback (`0x7871c0` → `0x787220`) that
//! draws it through the recursive draw (`0x78754c` → `0x76b3f0`): each frame's five layers, then
//! its shown children in link order, strata and level ignored. So the subtree paints at its
//! ScrollFrame's place, after that bucket's HIGHLIGHT textures and text, and a nested ScrollFrame's
//! child paints after its own HIGHLIGHT layer, before its children.
//!
//! The frame term is the client's live intrusive-list position: a frame relinks at its bucket's
//! tail when shown and when a shown frame's strata or level changes, and the later link draws on
//! top. [`crate::widget::WidgetArena`] keeps a counter bumped at those moments, which orders the
//! same within a bucket. The is-region bit is benilla's: the client has no frame drawable, and the
//! frame's own slot (a minimap or model pane, scissor) must precede its regions.
//!
//! A layer batch's textures do not keep that order: its End (`0x7731a0`) sorts them by texture
//! handle, which [`quad_batch`] groups for and `script::layer_batch` reproduces as far as it can.

use crate::widget::{FrameHandle, KindState, RegionHandle, RegionKind, WidgetArena};

// ── Strata: the top-level draw buckets ───────────────────────────────────────────────────────

/// The frame strata in draw order; `World` through `Tooltip` are the client's nine buckets (the
/// `CSimpleTop` constructor's loop at `0x764180`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Strata {
    /// The 3D world, behind everything.
    World = 0,
    Background = 1,
    Low = 2,
    Medium = 3,
    High = 4,
    Dialog = 5,
    Fullscreen = 6,
    FullscreenDialog = 7,
    Tooltip = 8,
}

impl Strata {
    /// Every stratum in draw order.
    pub const ALL: [Strata; 9] = [
        Strata::World,
        Strata::Background,
        Strata::Low,
        Strata::Medium,
        Strata::High,
        Strata::Dialog,
        Strata::Fullscreen,
        Strata::FullscreenDialog,
        Strata::Tooltip,
    ];

    /// The client bucket id.
    #[inline]
    pub const fn index(self) -> u8 {
        self as u8
    }
}

impl Default for Strata {
    /// The frame constructor writes 3 (`0x7690c2`).
    fn default() -> Strata {
        Strata::Medium
    }
}

// ── Draw layers ──────────────────────────────────────────────────────────────────────────────

/// The five draw layers, in draw order (the reference's five region lists near `0x1c0`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrawLayer {
    Background = 0,
    Border = 1,
    Artwork = 2,
    Overlay = 3,
    Highlight = 4,
}

impl DrawLayer {
    #[inline]
    pub const fn index(self) -> u8 {
        self as u8
    }
}

impl Default for DrawLayer {
    /// The layer of a texture or font string with no `<Layer>`.
    fn default() -> DrawLayer {
        DrawLayer::Artwork
    }
}

// ── ZKey: the packed total-order key ─────────────────────────────────────────────────────────
//
// Two 64-bit words, most significant first; one ascending sort gives the render list. The high
// word is the flat pass's key:
//
//   bits 60..=63 (4)   stratum           Strata::index()
//   bits 44..=59 (16)  frame level       u16
//   bits 41..=43 (3)   draw layer        DrawLayer::index()
//   bits 39..=40 (2)   batch rank        BatchRank: quad, text, callback
//   bits 21..=38 (18)  frame link-stamp  the (re)link sequence
//   bit  20      (1)   is-region         0 for the frame's own slot, 1 for a region
//   bits 12..=19 (8)   sub-level         i8 biased by +128
//   bits  0..=11 (12)  declaration seq   the region's index within its owner frame
//
// Stock content relies on the layer outranking the frame: a child is born at its parent's level
// plus 1 (`SetParent` `0x76ab10`, at `0x76ab65`), `MirrorTimer.xml:62` lowers the bar by 1 to tie
// its parent, and the tie lets the bar's ARTWORK fill draw under the parent's OVERLAY border.
//
// The sub-level is the engine's own ordering within a layer, set only for the EditBox caret: 1.12
// gives Lua and XML none (`0x76a860` takes only region and layer and appends at the tail, and
// `SetDrawLayer` `0x79a780` reads the layer alone).
//
// The low word is 0 for the flat pass. A scroll child's subtree takes its outermost ScrollFrame's
// HIGHLIGHT callback key as the high word and its recursive-draw order as the low word:
//
//   bits 26..=63 (38)  frame ordinal     preorder, from 1, a nested scroll child's subtree right
//                                        after its ScrollFrame and before that frame's children
//   bits 23..=25 (3)   draw layer        DrawLayer::index()
//   bits 21..=22 (2)   batch rank        BatchRank
//   bit  20      (1)   is-region         as in the high word
//   bits 12..=19 (8)   sub-level         as in the high word
//   bits  0..=11 (12)  declaration seq   as in the high word
//
// So inside the subtree the frame outranks the layer: `0x76b3f0` submits one frame's five batches
// (`0x76b4bf`–`0x76b4db`) before it recurses into the children (`0x76b4dd`).

const STRATUM_SHIFT: u32 = 60;
const LEVEL_SHIFT: u32 = 44;
const LAYER_SHIFT: u32 = 41;
const RANK_SHIFT: u32 = 39;
const INSERTION_SHIFT: u32 = 21;
const IS_REGION_SHIFT: u32 = 20;
const SUBLEVEL_SHIFT: u32 = 12;
const DECL_SHIFT: u32 = 0;

const NESTED_ORDINAL_SHIFT: u32 = 26;
const NESTED_LAYER_SHIFT: u32 = 23;
const NESTED_RANK_SHIFT: u32 = 21;

/// The link-stamp width; the arena renumbers its stamps at the cap.
pub(crate) const INSERTION_BITS: u32 = 18;
const DECL_BITS: u32 = 12;
/// One past the largest declaration seq the key holds; a frame renumbers its regions there.
pub(crate) const DECL_CAP: u32 = 1 << DECL_BITS;

/// Which of a layer batch's arrays an entry drains from, in `0x76fb00`'s order: quads at `+0x10`
/// (a texture appends at `0x7706e0`), a `CGxStringBatch` at `+0x18` (a font string at `0x772e50`)
/// and a render-callback list at `+0x1c` (ctor `0x772e80`). So in one `(strata, level, layer)`
/// every texture precedes every font string, and every font string every model scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BatchRank {
    Quad = 0,
    Text = 1,
    /// A render callback: a `Model` frame's scene, registered for ARTWORK only (`0x76d17f`), or a
    /// ScrollFrame's scroll child, for HIGHLIGHT only (`0x7871d5`).
    Callback = 2,
}

/// The packed draw-order key; its `Ord` is the client's draw order. The high word is the flat
/// pass's key and the low word a scroll subtree's order inside its ScrollFrame's callback, 0 for a
/// flat entry (the layout above).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZKey(u128);

/// The fields below the frame term of either word: a region's layer, rank, sub-level and
/// declaration seq at the given layer and rank shifts.
const fn entry_bits(
    layer_shift: u32,
    rank_shift: u32,
    layer: DrawLayer,
    rank: BatchRank,
    sub_level: i8,
    decl: u16,
) -> u64 {
    // Biased into 0..=255 so a higher sub-level sorts later.
    let sub_biased = (sub_level as i16 + 128) as u64;
    (1u64 << IS_REGION_SHIFT)
        | ((layer.index() as u64) << layer_shift)
        | ((rank as u64) << rank_shift)
        | (sub_biased << SUBLEVEL_SHIFT)
        | ((decl as u64) << DECL_SHIFT)
}

impl ZKey {
    #[inline]
    const fn words(hi: u64, lo: u64) -> ZKey {
        ZKey(((hi as u128) << 64) | lo as u128)
    }

    /// The flat pass's word.
    #[inline]
    const fn hi(self) -> u64 {
        (self.0 >> 64) as u64
    }

    /// The scroll subtree's word, 0 for a flat entry.
    #[inline]
    const fn lo(self) -> u64 {
        self.0 as u64
    }

    /// A frame's own entry; its region fields are zero, so it sorts before its regions.
    pub fn frame(strata: Strata, level: u16, insertion: u32) -> ZKey {
        debug_assert!(
            insertion < (1 << INSERTION_BITS),
            "frame insertion {insertion} exceeds {INSERTION_BITS} bits"
        );
        let bits = (u64::from(strata.index()) << STRATUM_SHIFT)
            | (u64::from(level) << LEVEL_SHIFT)
            | (u64::from(insertion) << INSERTION_SHIFT);
        ZKey::words(bits, 0)
    }

    /// A region's key; `strata`, `level` and `insertion` are its owner frame's, and
    /// `is_fontstring` picks the text rank.
    pub fn region(
        strata: Strata,
        level: u16,
        insertion: u32,
        layer: DrawLayer,
        sub_level: i8,
        is_fontstring: bool,
        decl: u16,
    ) -> ZKey {
        debug_assert!(
            u32::from(decl) < (1 << DECL_BITS),
            "region decl seq {decl} exceeds {DECL_BITS} bits"
        );
        let rank = if is_fontstring {
            BatchRank::Text
        } else {
            BatchRank::Quad
        };
        let bits = ZKey::frame(strata, level, insertion).hi()
            | entry_bits(LAYER_SHIFT, RANK_SHIFT, layer, rank, sub_level, decl);
        ZKey::words(bits, 0)
    }

    /// The frame at preorder `ordinal` (from 1) of the scroll subtree drawn at `slot`, its
    /// outermost ScrollFrame's HIGHLIGHT callback key.
    pub fn nested_frame(slot: ZKey, ordinal: u64) -> ZKey {
        debug_assert!(slot.lo() == 0, "a scroll subtree's slot is a flat key");
        debug_assert!(
            (1..1 << (64 - NESTED_ORDINAL_SHIFT)).contains(&ordinal),
            "scroll subtree ordinal {ordinal} out of range"
        );
        ZKey::words(slot.hi(), ordinal << NESTED_ORDINAL_SHIFT)
    }

    /// A region of the frame [`Self::nested_frame`] keys, ordered as [`Self::region`] orders one
    /// within its frame.
    pub fn nested_region(
        frame: ZKey,
        layer: DrawLayer,
        sub_level: i8,
        is_fontstring: bool,
        decl: u16,
    ) -> ZKey {
        let rank = if is_fontstring {
            BatchRank::Text
        } else {
            BatchRank::Quad
        };
        frame.with_entry(layer, rank, sub_level, decl)
    }

    /// This frame key's entry at `layer` and `rank`, in the word that orders the frame.
    #[inline]
    const fn with_entry(self, layer: DrawLayer, rank: BatchRank, sub_level: i8, decl: u16) -> ZKey {
        if self.lo() == 0 {
            ZKey::words(
                self.hi() | entry_bits(LAYER_SHIFT, RANK_SHIFT, layer, rank, sub_level, decl),
                0,
            )
        } else {
            ZKey::words(
                self.hi(),
                self.lo()
                    | entry_bits(
                        NESTED_LAYER_SHIFT,
                        NESTED_RANK_SHIFT,
                        layer,
                        rank,
                        sub_level,
                        decl,
                    ),
            )
        }
    }

    /// The key for a frame's own content, text with no region: its frame slot moved into its
    /// region band at `layer` with the text rank. The content is a ScrollingMessageFrame's lines,
    /// which the client draws as the frame's own font strings (ARTWORK with no `<Layer>`), so the
    /// frame's BACKGROUND art stays behind them.
    #[inline]
    #[must_use]
    pub const fn content(self, layer: DrawLayer) -> ZKey {
        self.with_entry(layer, BatchRank::Text, 0, 0)
    }

    /// The key for a frame's render callback at `layer`: its frame slot moved into its region band
    /// with the callback rank, after every texture and font string of the layer, as `0x76fb00`
    /// drains a layer batch's callbacks last. A `Model` frame's scene registers for ARTWORK
    /// (`0x76d160`) and a ScrollFrame's scroll child for HIGHLIGHT (`0x7871c0`).
    #[inline]
    #[must_use]
    pub const fn callback(self, layer: DrawLayer) -> ZKey {
        self.with_entry(layer, BatchRank::Callback, 0, 0)
    }

    #[inline]
    pub const fn raw(self) -> u128 {
        self.0
    }

    /// The flat pass's fields; a scroll subtree's entry gives its ScrollFrame's callback.
    #[inline]
    pub const fn parts(self) -> ZParts {
        unpack(self.0)
    }

    /// The key's `(strata, level)` plane in the flat pass.
    #[inline]
    const fn plane(self) -> u64 {
        self.hi() >> LEVEL_SHIFT
    }
}

/// A [`ZKey`]'s flat-pass fields, most significant first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZParts {
    /// [`Strata::index`].
    pub strata: u8,
    pub level: u16,
    /// [`DrawLayer::index`].
    pub layer: u8,
    pub rank: BatchRank,
    /// `rank == BatchRank::Text`.
    pub is_fontstring: bool,
    /// The owning frame's link-stamp.
    pub insertion: u32,
    pub is_region: bool,
    /// 0 for stock 1.12 content.
    pub sub_level: i8,
    /// Declaration order within the owning frame.
    pub decl: u16,
    /// Whether the key orders an entry of a scroll child's subtree, whose fields above are its
    /// outermost ScrollFrame's callback.
    pub in_scroll_child: bool,
}

/// A raw [`ZKey`]'s flat-pass fields, as [`crate::script::ExtractedQuad::z`] stores the key.
#[inline]
pub const fn unpack(raw: u128) -> ZParts {
    let hi = (raw >> 64) as u64;
    ZParts {
        strata: ((hi >> STRATUM_SHIFT) & 0xf) as u8,
        level: ((hi >> LEVEL_SHIFT) & 0xffff) as u16,
        layer: ((hi >> LAYER_SHIFT) & 0x7) as u8,
        rank: match (hi >> RANK_SHIFT) & 0x3 {
            0 => BatchRank::Quad,
            1 => BatchRank::Text,
            _ => BatchRank::Callback,
        },
        is_fontstring: (hi >> RANK_SHIFT) & 0x3 == 1,
        insertion: ((hi >> INSERTION_SHIFT) & ((1 << INSERTION_BITS) - 1)) as u32,
        is_region: (hi >> IS_REGION_SHIFT) & 1 == 1,
        sub_level: (((hi >> SUBLEVEL_SHIFT) & 0xff) as i16 - 128) as i8,
        decl: ((hi >> DECL_SHIFT) & ((1 << DECL_BITS) - 1)) as u16,
        in_scroll_child: raw as u64 != 0,
    }
}

/// The layer batch a raw key's texture quad fills, `None` for anything else: the flat pass's
/// `(strata, level, layer)` (`levelNode+0x1c`, every frame at the level sharing it), or, in a
/// scroll subtree, one frame's layer (`0x76b3f0` ends each frame's own five, `0x76b469`). The
/// keys of one batch's quads are contiguous among the regions, in the order the batch filled.
pub(crate) const fn quad_batch(raw: u128) -> Option<(u64, u64)> {
    let (hi, lo) = ((raw >> 64) as u64, raw as u64);
    let (word, rank_shift) = if lo == 0 {
        (hi, RANK_SHIFT)
    } else {
        (lo, NESTED_RANK_SHIFT)
    };
    let quad = (word >> rank_shift) & 0x3 == BatchRank::Quad as u64;
    if !quad || (word >> IS_REGION_SHIFT) & 1 == 0 {
        return None;
    }
    Some(if lo == 0 {
        (hi >> RANK_SHIFT, 0)
    } else {
        (hi, lo >> NESTED_RANK_SHIFT)
    })
}

// ── Traversal: the visible render list in painter order ──────────────────────────────────────

/// What a [`ZKey`] entry points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZTarget {
    /// The frame's own slot (a minimap or model pane, scissor).
    Frame(FrameHandle),
    Region(RegionHandle),
}

/// One entry of the draw list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawEntry {
    pub target: ZTarget,
    pub key: ZKey,
    /// The ScrollFrame whose callback draws this entry, the innermost one for a nested scroll
    /// child; `None` in the flat pass. Its rect clips the draw (`0x787220`).
    pub scope: Option<FrameHandle>,
}

/// The sorted draw list, shared so an unchanged frame hands out the same one.
pub type DrawList = std::sync::Arc<[DrawEntry]>;

/// Every shown frame with its own flat key, sorted: the hit test's planes.
pub type PickList = std::sync::Arc<[(FrameHandle, ZKey)]>;

/// [`traversal`]'s and [`pick_order`]'s memos.
#[derive(Default, Debug)]
pub struct OrderCache {
    draw: Memo<DrawEntry>,
    pick: Memo<(FrameHandle, ZKey)>,
}

/// One memo: the last walk, in arena order, and the sorted list built from it.
type Memo<T> = std::cell::RefCell<Option<(Vec<T>, std::sync::Arc<[T]>)>>;

/// How one frame's entries are keyed: by its own place in the flat pass, or by its preorder place
/// in a scroll subtree.
#[derive(Clone, Copy)]
enum FrameTerm {
    Flat {
        strata: Strata,
        level: u16,
        insertion: u32,
    },
    Nested(ZKey),
}

impl FrameTerm {
    fn frame(self) -> ZKey {
        match self {
            FrameTerm::Flat {
                strata,
                level,
                insertion,
            } => ZKey::frame(strata, level, insertion),
            FrameTerm::Nested(k) => k,
        }
    }

    fn region(self, layer: DrawLayer, sub_level: i8, is_fontstring: bool, decl: u16) -> ZKey {
        match self {
            FrameTerm::Flat {
                strata,
                level,
                insertion,
            } => ZKey::region(
                strata,
                level,
                insertion,
                layer,
                sub_level,
                is_fontstring,
                decl,
            ),
            FrameTerm::Nested(k) => ZKey::nested_region(k, layer, sub_level, is_fontstring, decl),
        }
    }
}

/// One frame's slot and regions.
fn emit_frame(
    arena: &WidgetArena,
    fh: FrameHandle,
    term: FrameTerm,
    scope: Option<FrameHandle>,
    out: &mut Vec<DrawEntry>,
) {
    let Some(frame) = arena.frame(fh) else {
        return;
    };
    out.push(DrawEntry {
        target: ZTarget::Frame(fh),
        key: term.frame(),
        scope,
    });
    for &rh in &frame.regions {
        let Some(region) = arena.region(rh) else {
            continue;
        };
        // A region orphaned by `SetParent(nil)` is in no draw layer until re-parented.
        if region.detached {
            continue;
        }
        let key = term.region(
            region.draw_layer,
            region.sub_level,
            matches!(region.kind, RegionKind::FontString),
            region.decl_seq as u16,
        );
        out.push(DrawEntry {
            target: ZTarget::Region(rh),
            key,
            scope,
        });
    }
}

/// The scroll child `sf`'s callback draws when it is shown (`0x787267`): a ScrollFrame's live
/// child still in the scroll draw.
///
/// Deviation: a child that left the scroll draw is drawn once, by the flat pass, where the client's
/// callback, which reads only the child's visibility, draws it a second time, clipped, at the
/// ScrollFrame's place. A `SetParent` of the child, of the ScrollFrame or of any ancestor of the
/// child to an unflagged parent clears `0x2000` on the whole moved subtree (`0x76abcb` →
/// `0x76a400`), so each of those reaches it.
fn scroll_child(arena: &WidgetArena, sf: FrameHandle) -> Option<FrameHandle> {
    let KindState::Scroll(s) = &arena.frame(sf)?.kind_state else {
        return None;
    };
    let child = s.child?;
    arena
        .frame(child)
        .is_some_and(|c| c.scroll_member)
        .then_some(child)
}

/// One scroll subtree's walk state.
struct Nest<'a> {
    arena: &'a WidgetArena,
    /// Its outermost ScrollFrame's HIGHLIGHT callback key.
    slot: ZKey,
    /// The last preorder ordinal given out.
    ordinal: u64,
    /// The frames on the current path: a scroll child that is its own ScrollFrame's ancestor
    /// would otherwise recurse forever.
    path: std::collections::HashSet<FrameHandle>,
}

impl Nest<'_> {
    /// The recursive draw `0x76b3f0` of `fh` under the ScrollFrame `scope`: the frame's own
    /// entries, then a ScrollFrame's scroll child (its HIGHLIGHT callback, the last of its
    /// batches), then each child in link order that is not itself a scroll child (`0x76b4dd`–
    /// `0x76b516`). Hidden frames are walked without drawing (`0x76b505` skips them), so they
    /// keep their ordinals and a Show or Hide renumbers nothing. `drawn` keeps a frame from
    /// drawing twice.
    fn frame(
        &mut self,
        fh: FrameHandle,
        scope: FrameHandle,
        draw: bool,
        drawn: &mut std::collections::HashSet<FrameHandle>,
        out: &mut Vec<DrawEntry>,
    ) {
        let arena = self.arena;
        let Some(frame) = arena.frame(fh) else {
            return;
        };
        let draw = draw && frame.effective_visible;
        if (draw && drawn.contains(&fh)) || !self.path.insert(fh) {
            return;
        }
        self.ordinal += 1;
        if draw {
            drawn.insert(fh);
            let key = ZKey::nested_frame(self.slot, self.ordinal);
            emit_frame(arena, fh, FrameTerm::Nested(key), Some(scope), out);
        }
        if let Some(child) = scroll_child(arena, fh) {
            self.frame(child, fh, draw, drawn, out);
        }
        for &c in &frame.children {
            if arena.frame(c).is_some_and(|cf| !cf.scroll_root) {
                self.frame(c, scope, draw, drawn, out);
            }
        }
        self.path.remove(&fh);
    }
}

/// Every [`DrawEntry`] of the draw list: the flat pass in arena order, each ScrollFrame's
/// subtree after it. Only frames under a ScrollFrame enter `drawn`.
fn walk(arena: &WidgetArena, out: &mut Vec<DrawEntry>) {
    let mut drawn = std::collections::HashSet::new();
    for (fh, frame) in arena.iter_frames() {
        // The flat pass skips a scroll child's subtree (`0x7659df`).
        if !frame.effective_visible || frame.scroll_member {
            continue;
        }
        let term = FrameTerm::Flat {
            strata: frame.strata,
            level: frame.level,
            insertion: frame.insertion_seq,
        };
        emit_frame(arena, fh, term, None, out);
        if let Some(child) = scroll_child(arena, fh) {
            let mut nest = Nest {
                arena,
                slot: term.frame().callback(DrawLayer::Highlight),
                ordinal: 0,
                path: std::collections::HashSet::new(),
            };
            nest.frame(child, fh, true, &mut drawn, out);
        }
    }
}

/// The render list in the client's order, sorted by [`ZKey`], with no two keys tied: the flat
/// pass `0x765650` over every shown frame outside a scroll child's subtree, and each such subtree
/// at its ScrollFrame's HIGHLIGHT callback. Only effective-visible frames contribute, as the
/// client skips a hidden frame's whole subtree (`0x76ae10`, `0x76ad50`). Every region of such a
/// frame is listed; hidden ones (`region+0xc4`, set by `0x77fcb0`) are dropped by
/// [`UiScript::extract`](crate::script::UiScript::extract), and where the client skips them at
/// draw time is untraced. An unchanged walk returns the last list.
pub fn traversal(arena: &WidgetArena) -> DrawList {
    memo(&arena.order_cache.draw, |v| walk(arena, v), |e| e.key)
}

/// Every shown frame by its own flat key, scroll children included: the mouse index holds a frame
/// by its strata and level whatever its draw (`0x764aa0`), and only the probe treats the scroll
/// subtree apart (`0x76b034`).
pub fn pick_order(arena: &WidgetArena) -> PickList {
    memo(
        &arena.order_cache.pick,
        |v| {
            for (fh, frame) in arena.iter_frames() {
                if frame.effective_visible {
                    v.push((
                        fh,
                        ZKey::frame(frame.strata, frame.level, frame.insertion_seq),
                    ));
                }
            }
        },
        |&(_, k)| k,
    )
}

/// Walk into a scratch vector and return the last sorted list when the walk is unchanged, else
/// sort the new walk and keep it.
fn memo<T: Clone + PartialEq>(
    cache: &Memo<T>,
    walk: impl FnOnce(&mut Vec<T>),
    key: impl Fn(&T) -> ZKey,
) -> std::sync::Arc<[T]> {
    let mut cache = cache.borrow_mut();
    let mut walked = Vec::with_capacity(cache.as_ref().map_or(0, |(prev, _)| prev.len()));
    walk(&mut walked);
    // An exact compare with the last walk, not a hash: a collision would draw in a wrong order.
    if let Some((prev, list)) = cache.as_ref() {
        if *prev == walked {
            return list.clone();
        }
    }
    let mut out = walked.clone();
    out.sort_by_key(|e| key(e));
    let list: std::sync::Arc<[T]> = out.into();
    walked.shrink_to_fit();
    *cache = Some((walked, list.clone()));
    list
}

// ── Hit-testing: the mouse-focus capture walk ────────────────────────────────────────────────

/// The frame the cursor captures, by the client's sweep `0x7660d0`: strata high to low, and in one
/// `(strata, level)` plane the earlier-linked frame first. The first frame that takes the mouse
/// ends the sweep whatever its handler does (`0x7663f7` overwrites the result of the call at
/// `0x7663f4`), so a disabled button still eats the click.
///
/// Equal levels go to the earlier link (`0x764aa0` appends after equals). A parent is linked before
/// its children (show at `0x76ae85`, then `0x76aeb6`; `SetFrameLevel` likewise), so it wins a tie
/// with its child, which `TargetFrame.lua:33` relies on; and the first-declared sibling wins, so a
/// full-area `<Button>` declared first, mouse-enabled by its constructor, swallows its window.
/// Drawing still puts the later link on top.
///
/// `sorted` is [`pick_order`]'s ascending list and `hits` says whether a frame is a candidate at
/// the cursor.
pub fn hit_test<F: Fn(FrameHandle) -> bool>(
    sorted: &[(FrameHandle, ZKey)],
    hits: F,
) -> Option<FrameHandle> {
    let mut end = sorted.len();
    while end > 0 {
        let p = sorted[end - 1].1.plane();
        let mut start = end;
        while start > 0 && sorted[start - 1].1.plane() == p {
            start -= 1;
        }
        // Planes descend; within one, the earlier-linked frame is probed first.
        for &(fh, _) in &sorted[start..end] {
            if hits(fh) {
                return Some(fh);
            }
        }
        end = start;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ZKey field ordering ────────────────────────────────────────────────────────────────────

    #[test]
    fn stratum_dominates_everything() {
        // A TOOLTIP frame at level 0 draws after a WORLD frame at max level.
        let lo = ZKey::frame(Strata::World, u16::MAX, 0);
        let hi = ZKey::frame(Strata::Tooltip, 0, 0);
        assert!(hi > lo);
    }

    #[test]
    fn level_then_insertion_within_stratum() {
        let a = ZKey::frame(Strata::Medium, 1, 999);
        let b = ZKey::frame(Strata::Medium, 2, 0);
        assert!(b > a, "higher level draws later regardless of insertion");
        let c = ZKey::frame(Strata::Medium, 1, 5);
        let d = ZKey::frame(Strata::Medium, 1, 6);
        assert!(d > c, "at equal level, later insertion draws later");
    }

    #[test]
    fn frame_precedes_its_regions() {
        let f = ZKey::frame(Strata::Medium, 3, 42);
        // Even a region in the lowest layer with the most-negative sub-level sorts after the frame.
        let r = ZKey::region(
            Strata::Medium,
            3,
            42,
            DrawLayer::Background,
            i8::MIN,
            false,
            0,
        );
        assert!(r > f);
    }

    #[test]
    fn layer_then_kind_then_frame_then_decl() {
        let mk = |layer, sub, fs, decl| ZKey::region(Strata::Medium, 3, 42, layer, sub, fs, decl);
        // The layer is the top term below strata and level.
        assert!(
            mk(DrawLayer::Border, i8::MIN, false, 0) > mk(DrawLayer::Background, 127, true, 4095)
        );
        // Within a layer the kind is next: every texture precedes every font string, whatever the
        // sub-level.
        assert!(
            mk(DrawLayer::Artwork, i8::MIN, true, 0) > mk(DrawLayer::Artwork, 127, false, 4095)
        );
        // Below kind, declaration order decides within one frame's layer.
        assert!(mk(DrawLayer::Artwork, 0, false, 2) > mk(DrawLayer::Artwork, 0, false, 1));
        // Sub-level orders below kind.
        assert!(mk(DrawLayer::Artwork, -1, false, 0) < mk(DrawLayer::Artwork, 0, false, 0));
    }

    /// The layer is bucket-wide: at one `(strata, level)` every frame's BACKGROUND draws before any
    /// frame's OVERLAY, so a frame's regions are not grouped behind it.
    #[test]
    fn the_layer_interleaves_frames_it_does_not_group_them() {
        let a_overlay = ZKey::region(Strata::Medium, 0, 10, DrawLayer::Overlay, 0, false, 0);
        let b_background = ZKey::region(Strata::Medium, 0, 11, DrawLayer::Background, 0, false, 0);
        assert!(
            b_background < a_overlay,
            "a later frame's BACKGROUND still draws before an earlier frame's OVERLAY"
        );

        // Same layer + same kind: the frame link-stamp decides, later on top.
        let a_art = ZKey::region(Strata::Medium, 0, 10, DrawLayer::Artwork, 0, false, 0);
        let b_art = ZKey::region(Strata::Medium, 0, 11, DrawLayer::Artwork, 0, false, 0);
        assert!(
            a_art < b_art,
            "at equal layer+kind, later link draws on top"
        );

        // Kind spans frames: every texture of a (strata, level, layer) precedes every font string.
        let a_text = ZKey::region(Strata::Medium, 0, 10, DrawLayer::Artwork, 0, true, 0);
        assert!(
            b_art < a_text,
            "a later frame's texture still precedes an earlier frame's fontstring"
        );
        // Every font string precedes a model's scene, whatever the link stamps.
        let earlier_model = ZKey::frame(Strata::Medium, 0, 9).callback(DrawLayer::Artwork);
        let later_text = ZKey::region(Strata::Medium, 0, 11, DrawLayer::Artwork, 0, true, 0);
        assert!(
            later_text < earlier_model,
            "an earlier-linked model's scene still draws after a later frame's ARTWORK text"
        );
        // The rank sits below the layer: an OVERLAY quad still covers an ARTWORK scene.
        let b_overlay = ZKey::region(Strata::Medium, 0, 11, DrawLayer::Overlay, 0, false, 0);
        assert!(
            earlier_model < b_overlay,
            "the layer outranks the batch rank"
        );
        // Two scenes in one bucket: registration order, which the link stamp carries.
        let later_model = ZKey::frame(Strata::Medium, 0, 12).callback(DrawLayer::Artwork);
        assert!(earlier_model < later_model);
        assert_eq!(unpack(earlier_model.raw()).rank, BatchRank::Callback);

        // A frame's own slot sorts just before its own BACKGROUND regions.
        let a_frame = ZKey::frame(Strata::Medium, 0, 10);
        let a_bg = ZKey::region(Strata::Medium, 0, 10, DrawLayer::Background, 0, false, 0);
        assert!(a_frame < a_bg);
    }

    // ── Traversal over a synthetic tree ────────────────────────────────────────────────────────

    use crate::widget::{FrameKind, WidgetArena};

    #[test]
    fn traversal_snapshot_over_a_small_tree() {
        let mut a = WidgetArena::new();

        // A DIALOG frame with two regions, a top-level MEDIUM frame (a MEDIUM child would be forced
        // to DIALOG) and a hidden frame.
        let dialog = a.create(FrameKind::Frame, Some("Dialog".into()), None);
        a.set_frame_strata(dialog, Strata::Dialog);
        let dlg_bg = a
            .create_region(dialog, RegionKind::Texture, DrawLayer::Background, 0)
            .unwrap();
        let dlg_text = a
            .create_region(dialog, RegionKind::FontString, DrawLayer::Artwork, 0)
            .unwrap();

        let medium = a.create(FrameKind::Frame, None, None); // MEDIUM, level 0
        let med_art = a
            .create_region(medium, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap();

        let hidden = a.create(FrameKind::Frame, None, None);
        let _hidden_rgn = a
            .create_region(hidden, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap();
        a.set_shown(hidden, false);

        let list: Vec<ZTarget> = traversal(&a).iter().map(|e| e.target).collect();

        // MEDIUM first; the hidden frame is absent.
        assert_eq!(
            list,
            vec![
                ZTarget::Frame(medium),
                ZTarget::Region(med_art),
                ZTarget::Frame(dialog),
                ZTarget::Region(dlg_bg),
                ZTarget::Region(dlg_text),
            ]
        );
    }

    /// Showing a frame relinks it at its bucket's tail (`0x76ae10`): `MiniMapTrackingFrame`,
    /// declared hidden before `MinimapBackdrop` (`Minimap.xml:109`, `:415`), draws over the
    /// backdrop once shown. A strata or level change relinks a shown frame, not a hidden one.
    #[test]
    fn showing_a_frame_moves_it_to_its_buckets_tail() {
        let mut a = WidgetArena::new();
        let tracking = a.create(FrameKind::Frame, Some("Tracking".into()), None);
        let backdrop = a.create(FrameKind::Frame, Some("Backdrop".into()), None);

        let order =
            |a: &WidgetArena| -> Vec<ZTarget> { traversal(a).iter().map(|e| e.target).collect() };
        // Declaration order while both start shown: tracking before backdrop.
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(tracking), ZTarget::Frame(backdrop)]
        );

        // Hide then show: tracking relinks at the tail, above the backdrop.
        a.set_shown(tracking, false);
        a.set_shown(tracking, true);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(backdrop), ZTarget::Frame(tracking)]
        );

        // Showing a shown frame relinks nothing.
        a.set_shown(backdrop, true);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(backdrop), ZTarget::Frame(tracking)]
        );

        // A shown frame's level round trip relinks it at the tail.
        a.set_frame_level(backdrop, 1, false);
        a.set_frame_level(backdrop, 0, false);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(tracking), ZTarget::Frame(backdrop)]
        );

        // The same round trip while hidden moves nothing; the show relinks it.
        a.set_shown(tracking, false);
        a.set_frame_level(tracking, 1, false);
        a.set_frame_level(tracking, 0, false);
        a.set_shown(tracking, true);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(backdrop), ZTarget::Frame(tracking)]
        );
    }

    // ── Scroll children: the ScrollFrame's callback draws them ────────────────────────────────

    fn tex(a: &mut WidgetArena, f: FrameHandle, layer: DrawLayer) -> RegionHandle {
        a.create_region(f, RegionKind::Texture, layer, 0).unwrap()
    }

    fn pos(list: &[ZTarget], t: ZTarget) -> usize {
        list.iter()
            .position(|&x| x == t)
            .unwrap_or_else(|| panic!("{t:?} is not drawn"))
    }

    fn targets(a: &WidgetArena) -> Vec<ZTarget> {
        traversal(a).iter().map(|e| e.target).collect()
    }

    /// The create screen's info panel (`CharacterCreate.xml:212`–`:330`): an icon frame and a
    /// ScrollFrame under one panel, the icon raised one level in its `OnLoad` (`:238`), which ties
    /// it with the scroll child born one above the ScrollFrame. The scroll child draws in the
    /// ScrollFrame's HIGHLIGHT callback (`0x7871c0` → `0x787220`), at the ScrollFrame's level, so
    /// the icon covers the text; by its own level the text would draw over the icon.
    #[test]
    fn a_scroll_child_draws_at_its_scroll_frames_place_under_a_raised_sibling() {
        let mut a = WidgetArena::new();
        // The arena's ctor leaves level 0; the levels are `SetParent`'s, the parent's plus one.
        let panel = a.create(FrameKind::Frame, None, None);
        let icon = a.create(FrameKind::Frame, None, Some(panel));
        a.set_frame_level(icon, 1, false);
        let sf = a.create(FrameKind::ScrollFrame, None, Some(panel));
        a.set_frame_level(sf, 1, false);
        let sf_art = tex(&mut a, sf, DrawLayer::Background);
        let sf_highlight = tex(&mut a, sf, DrawLayer::Highlight);
        let child = a.create(FrameKind::Frame, None, Some(sf));
        a.set_frame_level(child, 2, false);
        let text = a
            .create_region(child, RegionKind::FontString, DrawLayer::Background, 0)
            .unwrap();
        // Linked after the ScrollFrame at its level: its HIGHLIGHT quads share the batch whose
        // callbacks `0x76fb00` drains last.
        let later = a.create(FrameKind::Frame, None, Some(panel));
        a.set_frame_level(later, 1, false);
        let later_highlight = tex(&mut a, later, DrawLayer::Highlight);
        a.set_scroll_child(sf, Some(child));
        let icon_art = tex(&mut a, icon, DrawLayer::Background);
        a.set_frame_level(icon, 2, false);
        assert_eq!(a.frame(child).unwrap().level, 2, "the tie the raise makes");

        let list = targets(&a);
        assert!(
            pos(&list, ZTarget::Region(text)) < pos(&list, ZTarget::Region(icon_art)),
            "the raised icon covers the scroll child's text"
        );
        for under in [sf_art, sf_highlight, later_highlight] {
            assert!(
                pos(&list, ZTarget::Region(under)) < pos(&list, ZTarget::Frame(child)),
                "the callback draws after its bucket's HIGHLIGHT quads"
            );
        }
        let key = traversal(&a)
            .iter()
            .find(|e| e.target == ZTarget::Region(text))
            .map(|e| (e.key, e.scope))
            .unwrap();
        assert_eq!(key.1, Some(sf), "the ScrollFrame's rect clips it");
        let p = key.0.parts();
        assert!(p.in_scroll_child);
        assert_eq!((p.level, p.layer, p.rank), (1, 4, BatchRank::Callback));
    }

    /// Inside the subtree the recursive draw `0x76b3f0` orders frames, not levels or strata: each
    /// frame's five layers (`0x76b4bf`–`0x76b4db`), then its shown children in link order
    /// (`0x76b4dd`), a nested ScrollFrame's child at that frame's HIGHLIGHT callback, before its
    /// other children, and a scroll child reached as a child skipped (`0x76b509`).
    #[test]
    fn a_scroll_subtree_keeps_its_recursive_draw_order() {
        let mut a = WidgetArena::new();
        let sf = a.create(FrameKind::ScrollFrame, None, None);
        let child = a.create(FrameKind::Frame, None, Some(sf));
        a.set_scroll_child(sf, Some(child));
        let child_overlay = tex(&mut a, child, DrawLayer::Overlay);
        let child_art = tex(&mut a, child, DrawLayer::Artwork);
        // Lowered under its parent, and another stratum: neither moves it in the subtree.
        let first = a.create(FrameKind::Frame, None, Some(child));
        a.set_frame_level(first, 0, false);
        let first_bg = tex(&mut a, first, DrawLayer::Background);
        let grand = a.create(FrameKind::Frame, None, Some(first));
        let grand_bg = tex(&mut a, grand, DrawLayer::Background);
        let inner = a.create(FrameKind::ScrollFrame, None, Some(child));
        let inner_highlight = tex(&mut a, inner, DrawLayer::Highlight);
        let inner_other = a.create(FrameKind::Frame, None, Some(inner));
        let inner_other_bg = tex(&mut a, inner_other, DrawLayer::Background);
        let inner_child = a.create(FrameKind::Frame, None, Some(inner));
        a.set_scroll_child(inner, Some(inner_child));
        let inner_child_bg = tex(&mut a, inner_child, DrawLayer::Background);
        let last = a.create(FrameKind::Frame, None, Some(child));
        a.set_frame_strata(last, Strata::Background);
        let last_bg = tex(&mut a, last, DrawLayer::Background);
        let hidden = a.create(FrameKind::Frame, None, Some(child));
        tex(&mut a, hidden, DrawLayer::Background);
        a.set_shown(hidden, false);

        let list = targets(&a);
        let at = |t| pos(&list, t);
        let want = [
            ZTarget::Frame(child),
            ZTarget::Region(child_art),
            ZTarget::Region(child_overlay),
            ZTarget::Frame(first),
            ZTarget::Region(first_bg),
            ZTarget::Region(grand_bg),
            ZTarget::Frame(inner),
            ZTarget::Region(inner_highlight),
            ZTarget::Region(inner_child_bg),
            ZTarget::Region(inner_other_bg),
            ZTarget::Region(last_bg),
        ];
        for w in want.windows(2) {
            assert!(at(w[0]) < at(w[1]), "{:?} draws before {:?}", w[0], w[1]);
        }
        assert!(!list.contains(&ZTarget::Frame(hidden)));
        assert_eq!(
            list.iter()
                .filter(|&&t| t == ZTarget::Frame(inner_child))
                .count(),
            1,
            "the nested scroll child draws once, at its ScrollFrame's callback"
        );
        let scope_of = |t| traversal(&a).iter().find(|e| e.target == t).unwrap().scope;
        assert_eq!(scope_of(ZTarget::Region(inner_child_bg)), Some(inner));
        assert_eq!(scope_of(ZTarget::Region(inner_other_bg)), Some(sf));
        assert_eq!(scope_of(ZTarget::Frame(sf)), None);
    }

    /// The scroll flag's life (`0x76a400`): set on the child's subtree by `SetScrollChild`, cleared
    /// on the old child when another replaces it (`0x786cf4`), taken from the parent by a frame
    /// created or re-parented under it (`0x76abcb`), so a child moved out draws by its own level.
    #[test]
    fn the_scroll_flag_follows_set_scroll_child_and_set_parent() {
        let mut a = WidgetArena::new();
        let sf = a.create(FrameKind::ScrollFrame, None, None);
        let child = a.create(FrameKind::Frame, None, Some(sf));
        let kept = a.create(FrameKind::Frame, None, Some(child));
        a.set_scroll_child(sf, Some(child));
        let born = a.create(FrameKind::Frame, None, Some(kept));
        let outside = a.create(FrameKind::Frame, None, None);
        let joined = a.create(FrameKind::Frame, None, Some(outside));
        a.set_parent(joined, Some(kept));
        for f in [child, kept, born, joined] {
            assert!(a.frame(f).unwrap().scroll_member, "{f:?} is in the subtree");
        }
        assert!(a.frame(child).unwrap().scroll_root);
        assert!(!a.frame(kept).unwrap().scroll_root);
        let nested = |a: &WidgetArena, f| {
            traversal(a)
                .iter()
                .filter(|e| e.target == ZTarget::Frame(f))
                .map(|e| e.key.parts().in_scroll_child)
                .collect::<Vec<_>>()
        };
        assert_eq!(nested(&a, born), vec![true]);

        // Out by re-parenting: the flat pass draws it, once, though the ScrollFrame still holds it.
        a.set_parent(joined, Some(outside));
        assert!(!a.frame(joined).unwrap().scroll_member);
        assert_eq!(nested(&a, joined), vec![false]);
        a.set_parent(child, None);
        assert!(!a.frame(born).unwrap().scroll_member);
        assert!(
            a.frame(child).unwrap().scroll_root,
            "SetParent leaves the 0x4000 mark"
        );
        assert_eq!(nested(&a, child), vec![false]);
        assert_eq!(nested(&a, born), vec![false]);

        // Replaced: the old child's subtree is cleared whatever its parent.
        a.set_parent(child, Some(sf));
        a.set_scroll_child(sf, Some(child));
        assert!(a.frame(born).unwrap().scroll_member);
        let other = a.create(FrameKind::Frame, None, Some(sf));
        a.set_scroll_child(sf, Some(other));
        for f in [child, kept, born] {
            let fr = a.frame(f).unwrap();
            assert!(
                !fr.scroll_member && !fr.scroll_root,
                "{f:?} left the subtree"
            );
        }
        assert_eq!(nested(&a, born), vec![false]);
        assert_eq!(nested(&a, other), vec![true]);
    }

    /// A Show or Hide inside a scroll subtree moves no other entry's key: a hidden frame keeps its
    /// preorder place, so the host's splice still pairs what follows it.
    #[test]
    fn a_show_or_hide_in_a_scroll_subtree_keeps_the_other_keys() {
        let mut a = WidgetArena::new();
        let sf = a.create(FrameKind::ScrollFrame, None, None);
        let child = a.create(FrameKind::Frame, None, Some(sf));
        a.set_scroll_child(sf, Some(child));
        let first = a.create(FrameKind::Frame, None, Some(child));
        tex(&mut a, first, DrawLayer::Artwork);
        let toggled = a.create(FrameKind::Frame, None, Some(child));
        let under = a.create(FrameKind::Frame, None, Some(toggled));
        tex(&mut a, under, DrawLayer::Artwork);
        let last = a.create(FrameKind::Frame, None, Some(child));
        tex(&mut a, last, DrawLayer::Artwork);
        let keys = |a: &WidgetArena| {
            traversal(a)
                .iter()
                .filter(|e| {
                    let owner = match e.target {
                        ZTarget::Frame(f) => Some(f),
                        ZTarget::Region(r) => a.region(r).map(|r| r.owner),
                    };
                    owner != Some(toggled) && owner != Some(under)
                })
                .map(|e| (e.target, e.key))
                .collect::<Vec<_>>()
        };
        let shown = keys(&a);
        a.set_shown(toggled, false);
        assert!(!targets(&a).contains(&ZTarget::Frame(under)));
        assert_eq!(keys(&a), shown, "hidden, the rest keep their keys");
        a.set_shown(toggled, true);
        assert_eq!(keys(&a), shown, "shown again, likewise");
    }

    /// A scroll child that contains its own ScrollFrame draws nothing rather than recursing:
    /// the flag hides both from the flat pass, and no callback is reached.
    #[test]
    fn a_scroll_child_holding_its_scroll_frame_does_not_recurse() {
        let mut a = WidgetArena::new();
        let outer = a.create(FrameKind::Frame, None, None);
        let sf = a.create(FrameKind::ScrollFrame, None, Some(outer));
        a.set_scroll_child(sf, Some(outer));
        assert!(targets(&a).is_empty());
        // A nested ScrollFrame whose child is the outer scroll child: drawn once.
        let mut a = WidgetArena::new();
        let sf = a.create(FrameKind::ScrollFrame, None, None);
        let child = a.create(FrameKind::Frame, None, Some(sf));
        a.set_scroll_child(sf, Some(child));
        let inner = a.create(FrameKind::ScrollFrame, None, Some(child));
        if let Some(KindState::Scroll(s)) = a.frame_mut(inner).map(|f| &mut f.kind_state) {
            s.child = Some(child);
        }
        let list = targets(&a);
        assert_eq!(
            list.iter().filter(|&&t| t == ZTarget::Frame(child)).count(),
            1
        );
    }

    /// The hit test's planes are each frame's own strata and level, scroll children included:
    /// the mouse index (`0x764aa0`) knows nothing of the draw's callback.
    #[test]
    fn the_pick_order_keeps_a_scroll_childs_own_level() {
        let mut a = WidgetArena::new();
        let sf = a.create(FrameKind::ScrollFrame, None, None);
        let over = a.create(FrameKind::Frame, None, None);
        a.set_frame_level(over, 1, false);
        let child = a.create(FrameKind::Frame, None, Some(sf));
        a.set_scroll_child(sf, Some(child));
        let button = a.create(FrameKind::Button, None, Some(child));
        a.set_frame_level(button, 2, false);
        let picks = pick_order(&a);
        assert_eq!(hit_test(&picks, |_| true), Some(button));
        let list = targets(&a);
        assert!(pos(&list, ZTarget::Frame(button)) < pos(&list, ZTarget::Frame(over)));
    }

    // ── Hit-testing (pure core) ─────────────────────────────────────────────────────────────────

    #[test]
    fn hit_test_returns_topmost_matching_frame() {
        let mut a = WidgetArena::new();
        // `top` (DIALOG) draws above `bottom` (MEDIUM).
        let bottom = a.create(FrameKind::Frame, None, None);
        let top = a.create(FrameKind::Frame, None, None);
        a.set_frame_strata(top, Strata::Dialog);

        let sorted = pick_order(&a);
        assert_eq!(hit_test(&sorted, |_| true), Some(top));
        // A `top` that does not take the mouse lets `bottom` win.
        assert_eq!(hit_test(&sorted, |fh| fh == bottom), Some(bottom));
        assert_eq!(hit_test(&sorted, |_| false), None);
    }
}
