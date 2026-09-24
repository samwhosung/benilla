//! The draw-order primitive — the strata/layer vocabulary and the packed total-order key
//! (`ZKey`), plus the visible-tree [`traversal`] that realizes the client's painter order.
//!
//! ## Ground truth
//!
//! The 1.12.1 client draws in a **flat painter's order**, *not* a hierarchical walk: every visible
//! frame — children included — is an independent entry in a per-`(strata, level)` bucket
//! (`root+0xcd4`). A child frame with a *lower* strata/level than its
//! parent therefore draws *before* the parent — the order is global, keyed on the tuple below, not
//! on the tree shape.
//!
//! **Inside one `(strata, level)` bucket, the DRAW LAYER outranks the frame.** The five render
//! batches belong to the level node (`levelNode+0x1c`), not to any frame, and the emitter
//! `0x765920` loops `layer = 0..4` outside and frames inside. So a bucket emits every frame's
//! BACKGROUND, then every frame's BORDER, and so on — regions are *not* grouped behind their owning
//! frame. Within one layer the same is true of kind: the batch holds a quad sub-array and a text
//! sub-array, and `0x76fb00` drains all quads before any text, so **all** textures of a
//! `(strata, level, layer)` precede **all** its font strings.
//!
//! ## The total order (most- to least-significant)
//!
//! `(stratum, frame level, draw layer, batch rank quad<text<callback, frame link-stamp, is-region,
//! sub-level, declaration order)` — packed big-endian-by-priority into one sortable [`ZKey`] (`u64`), so the
//! whole render list is produced by one sort on the key. The `is-region` bit is benilla's own: the
//! binary has no frame-level drawable (only regions draw), so it sits *below* the layer key purely
//! to keep a frame's backdrop/scissor slot ahead of that frame's own regions.
//!
//! The **link-stamp** is the client's live intrusive-list position, not a creation index: a frame is
//! (re)linked at its bucket on becoming visible, on a strata change, and on a *changing* level set
//! — later link = drawn on top. A monotonic global counter bumped at those same moments is
//! order-equivalent within any one bucket, which is what [`crate::widget::WidgetArena`] keeps.
//!
//! **Below the layer, the frame term is exact for font strings and a deterministic stand-in for
//! textures.** The client re-sorts each layer's quad array by *texture handle* (`0x7731a0`) and
//! never sorts the text sub-array. A texture handle is an internal allocation our texture manager
//! assigns differently, so we reproduce the client's pre-sort array instead — identical wherever a
//! layer's quads share a texture or don't overlap. **No content may depend on the relative order of
//! two overlapping textures within one layer.**
//!
//! ## Era deltas (designed-for, not invented)
//!
//! - **`BLIZZARD` stratum** — the modern engine adds exactly one stratum *above* `TOOLTIP`
//!   (decision 0068 §"strict superset"; the 1.12 `UI.xsd`↔Era diff). It is the last [`Strata`]
//!   variant; 1.12 content never selects it.
//! - **`textureSubLevel`** — an i8 ordering knob *within* a draw layer, first-class in Era. There
//!   is **no sub-level in 5875 at all** (`0x76a860` takes only `(region, layer)` and head-inserts
//!   with no comparator; the per-layer header is a plain 0xc-byte triple with nowhere to keep a
//!   sort key). The `sub_level` field stays as the Era delta and is inert for 1.12 content.

use crate::widget::{FrameHandle, RegionHandle, RegionKind, WidgetArena};

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Strata — the top-level draw buckets
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The frame strata, in draw order (low → high). Variants 0..=8 are the client's nine
/// buckets, byte-verified in the `CSimpleTop` ctor's 9-loop (`0x764180`): WORLD, BACKGROUND,
/// LOW, MEDIUM (the default, id 3), HIGH, DIALOG, FULLSCREEN, FULLSCREEN_DIALOG, TOOLTIP. `BLIZZARD`
/// (id 9) is the one stratum the modern engine adds *above* TOOLTIP — an
/// extension point, never selected by 1.12 content.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Strata {
    /// id 0 — behind everything (the 3D world layer).
    World = 0,
    /// id 1.
    Background = 1,
    /// id 2.
    Low = 2,
    /// id 3 — the client default (`frameStrata` ctor value `3` at `0x7690c2`).
    Medium = 3,
    /// id 4.
    High = 4,
    /// id 5.
    Dialog = 5,
    /// id 6.
    Fullscreen = 6,
    /// id 7.
    FullscreenDialog = 7,
    /// id 8 — topmost in 1.12.
    Tooltip = 8,
    /// id 9 — **Era addition**, drawn above `Tooltip`. No 1.12 content uses it.
    Blizzard = 9,
}

impl Strata {
    /// Every stratum in draw order (low → high).
    pub const ALL: [Strata; 10] = [
        Strata::World,
        Strata::Background,
        Strata::Low,
        Strata::Medium,
        Strata::High,
        Strata::Dialog,
        Strata::Fullscreen,
        Strata::FullscreenDialog,
        Strata::Tooltip,
        Strata::Blizzard,
    ];

    /// The client bucket id (0..=9).
    #[inline]
    pub const fn index(self) -> u8 {
        self as u8
    }
}

impl Default for Strata {
    /// `MEDIUM` — the client's default `frameStrata` (ctor writes `3` at `0x7690c2`).
    fn default() -> Strata {
        Strata::Medium
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Draw layers — the within-frame region order
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The five draw layers a region sits in, in draw order (the reference's five region lists
/// ~`0x1c0`): BACKGROUND, BORDER, ARTWORK, OVERLAY, HIGHLIGHT.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrawLayer {
    /// Drawn first (behind the frame's other regions).
    Background = 0,
    Border = 1,
    /// The default layer for a texture/fontstring with no explicit `<Layer>`.
    Artwork = 2,
    Overlay = 3,
    /// Drawn last (mouse-over highlights).
    Highlight = 4,
}

impl DrawLayer {
    /// The layer index (0..=4).
    #[inline]
    pub const fn index(self) -> u8 {
        self as u8
    }
}

impl Default for DrawLayer {
    /// `ARTWORK` — the layer a texture/fontstring with no explicit `<Layer>` lands in.
    fn default() -> DrawLayer {
        DrawLayer::Artwork
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// ZKey — the packed total-order key
// ─────────────────────────────────────────────────────────────────────────────────────────────
//
// Bit layout (u64), most-significant field = highest draw priority. The whole render list is a
// single ascending sort on this key.
//
//   bits 60..=63 (4)   stratum          Strata::index() 0..=9   (< 16)
//   bits 44..=59 (16)  frame level      u16                     (< 65536)
//   bits 41..=43 (3)   draw layer       DrawLayer::index() 0..=4 (< 8)
//   bit  40      (1)   fontstring       0 = texture, 1 = fontstring — BUCKET-WIDE, above the frame
//   bits 21..=39 (19)  frame link-stamp monotonic (re)link seq   (< 524288)
//   bit  20      (1)   is-region        0 = the frame itself, 1 = one of its regions
//   bits 12..=19 (8)   sub-level        i8 biased by +128        (0..=255) — INERT on 1.12
//   bits  0..=11 (12)  declaration seq  region index within its owner frame (< 4096)
//
// ## Why the draw layer outranks the frame (§5-VERIFIED)
//
// **The five render batches belong to the LEVEL NODE, not to the frame** (`levelNode+0x1c`, five
// 0x30-byte batches), and the emitter `0x765920` loops `layer = 0..4` on the OUTSIDE and frames on
// the inside. So the draw layer is a **bucket-wide** key: every frame sharing a `(strata, level)`
// emits its BACKGROUND regions, then every frame its BORDER, then ARTWORK, then OVERLAY, then
// HIGHLIGHT. It is not a within-frame ordering — which is what benilla modeled until 0884, with
// the frame term ranked above the layer so a frame's regions stayed glued behind it.
//
// That inversion is what put the mirror timer's blue fill over its own border art and caption
// (director report, 2026-08-02). The reference builds those bars as a Frame whose OVERLAY layer
// carries the border + caption, with a child StatusBar whose `<BarTexture>` takes the default
// **ARTWORK** layer and whose only OnLoad is `SetFrameLevel(GetFrameLevel() - 1)`. That line does
// **not** put the child below the parent: a child is born at `parent.level + 1` (`SetParent
// 0x76ab10` @`0x76ab65`), so the `-1` lands it at *exactly* the parent's level. Its whole job is to
// **create the tie**, so the layer key can decide — ARTWORK(2) before OVERLAY(3), fill under
// chrome. Ranked below the frame, there was no tie to resolve and the child always trailed.
//
// **`texture < fontstring` is bucket-wide too, and outranks the frame.** A layer's batch holds two
// independent sub-arrays — the quad array `[batch+0x10]` (every `CSimpleTexture` appends via
// `0x7706e0` → `0x772fd0`) and the text sub-batch `[batch+0x18]` (`0x772e50` → `0x773080`) — and
// `0x76fb00` drains the WHOLE quad array before it touches the text one. So for two frames A, B at
// one `(strata, level, layer)`, each with a texture and a font string, the client emits
// `A.tex, B.tex, A.text, B.text` — never `A.tex, A.text, B.tex, B.text`.
//
// **The frame link-stamp below that is exact for font strings and a deterministic stand-in for
// textures.** The client re-sorts a layer's quad array by *texture handle* (`0x7731a0`'s qsort,
// comparator `0x7731c0`) and never sorts the text sub-batch. The handle is an internal allocation
// our texture manager assigns differently, so reproducing that sort is neither achievable nor
// desirable; we reproduce the client's PRE-sort array, which agrees wherever a layer's quads share
// a texture or don't overlap (the common case). **No content may depend on the relative order of
// two overlapping textures within one layer** — that is not a fidelity invariant.
//
// A frame's own entry zeroes the layer and everything below `is-region`, so it precedes its own
// BACKGROUND regions. That slot is benilla's backdrop/scissor hook, not the client's — in the
// binary a frame has no drawable of its own — so it sits *below* the layer key, never above it.
//
// **`sub-level` is inert on 1.12**: there is no sub-level in 5875 at all (`0x76a860` takes only
// `(region, layer)`, head-inserts with no comparator, and the per-layer header is a plain 0xc-byte
// triple with nowhere to keep a sort key). The field stays as the modeled Era delta; no 1.12
// content sets it, so it is always the `0` bias.

const STRATUM_SHIFT: u32 = 60;
const LEVEL_SHIFT: u32 = 44;
const LAYER_SHIFT: u32 = 41;
/// The batch RANK within a `(strata, level, layer)`: the three sub-arrays a layer batch carries and
/// the order `0x76fb00` drains them — its quads (`+0x10`), its string batch (`+0x18`), then its
/// render-callback list (`+0x1c`).
/// Two bits at 39..40, where the one font-string bit used to be; the link stamp below lost a bit
/// for it (18, from 19 — the arena renumbers at the cap either way).
const RANK_SHIFT: u32 = 39;
const INSERTION_SHIFT: u32 = 21;
const IS_REGION_SHIFT: u32 = 20;
const SUBLEVEL_SHIFT: u32 = 12;
const DECL_SHIFT: u32 = 0;

pub(crate) const INSERTION_BITS: u32 = 18;
const DECL_BITS: u32 = 12;

/// Which of a layer batch's three sub-arrays an entry drains from — in the order `0x76fb00` drains
/// them. The client keeps them as three arrays on one batch object (ctor `0x772e80`: quads at
/// `+0x10`, the `CGxStringBatch` at `+0x18`, a `RENDERCALLBACKNODE` list at `+0x1c`) and empties
/// them in this order, so within one `(strata, level, layer)` every texture precedes every font
/// string, and every font string precedes every model scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BatchRank {
    /// A texture quad — drained first.
    Quad = 0,
    /// A font string — the string batch, drained second.
    Text = 1,
    /// A `Model` frame's scene — the callback list, drained last. Registered for ARTWORK only
    /// (`0x76d17f cmp ebx,2`), which is why [`ZKey::callback`] takes the layer it is asked for
    /// but a model's is always `ARTWORK`.
    Callback = 2,
}

/// The packed draw-order key: a single `u64` whose natural `Ord` *is* the client's total draw
/// order. Build one with [`ZKey::frame`] or [`ZKey::region`]; sort ascending to get painter order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZKey(u64);

impl ZKey {
    /// The key for a **frame's own** entry — every region-level field is zero, so this sorts before
    /// all of the frame's regions.
    pub fn frame(strata: Strata, level: u16, insertion: u32) -> ZKey {
        debug_assert!(
            insertion < (1 << INSERTION_BITS),
            "frame insertion {insertion} exceeds {INSERTION_BITS} bits"
        );
        let bits = (u64::from(strata.index()) << STRATUM_SHIFT)
            | (u64::from(level) << LEVEL_SHIFT)
            | (u64::from(insertion) << INSERTION_SHIFT);
        ZKey(bits)
    }

    /// The key for one **region** of a frame. `strata`/`level`/`insertion` are the *owner frame's*
    /// (so the region stays grouped behind its frame); `is_fontstring` places textures before
    /// fontstrings at equal (layer, sub-level).
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
            insertion < (1 << INSERTION_BITS),
            "frame insertion {insertion} exceeds {INSERTION_BITS} bits"
        );
        debug_assert!(
            u32::from(decl) < (1 << DECL_BITS),
            "region decl seq {decl} exceeds {DECL_BITS} bits"
        );
        // sub_level (i8, -128..=127) biased into 0..=255 so higher sub-level sorts later.
        let sub_biased = (i16::from(sub_level) + 128) as u64;
        let bits = (u64::from(strata.index()) << STRATUM_SHIFT)
            | (u64::from(level) << LEVEL_SHIFT)
            | (u64::from(insertion) << INSERTION_SHIFT)
            | (1u64 << IS_REGION_SHIFT)
            | (u64::from(layer.index()) << LAYER_SHIFT)
            | (sub_biased << SUBLEVEL_SHIFT)
            | ((if is_fontstring {
                BatchRank::Text
            } else {
                BatchRank::Quad
            }) as u64)
                << RANK_SHIFT
            | (u64::from(decl) << DECL_SHIFT);
        ZKey(bits)
    }

    /// The key a frame's own **content** draws at: its bare [`ZKey::frame`] slot promoted into its
    /// region band at `layer` — behind that layer's textures, ahead of its declared font strings.
    ///
    /// Content is what a widget kind draws *itself*, with no region to hang it on — today only a
    /// ScrollingMessageFrame's ring lines, which the client materializes as the frame's own font
    /// strings (its `<FontString>` child is their font instance, and a font string with no
    /// `<Layer>` is `ARTWORK`). At the bare frame slot they sorted before **all** of the frame's
    /// regions, so the chat window's own `$parentBackground` — a `BACKGROUND` texture, faded in
    /// on hover — painted over the messages and dimmed them (director, 2026-07-26; the white
    /// text measured 192, exactly `255 × (1 − 0.25)`, the box's own alpha). The frame slot stays
    /// what it was — the backdrop/scissor setup that must precede everything the frame owns.
    #[inline]
    #[must_use]
    pub const fn content(self, layer: DrawLayer) -> ZKey {
        ZKey(
            self.0
                | (1u64 << IS_REGION_SHIFT)
                | ((layer.index() as u64) << LAYER_SHIFT)
                // Sub-level 0 in the same +128 bias `region` uses, and the text rank: content is
                // text, so it takes the same textures-draw-first slot a font string would.
                | (128u64 << SUBLEVEL_SHIFT)
                | ((BatchRank::Text as u64) << RANK_SHIFT),
        )
    }

    /// The key a `Model` frame's **scene** draws at: its bare [`ZKey::frame`] slot promoted into
    /// its region band at `layer` with the CALLBACK rank — after every texture and every font
    /// string of that `(strata, level, layer)`, whatever their link stamps. That is the client's
    /// own place for it: `0x76d160` registers the model's render callback into the layer batch,
    /// and `0x76fb00` drains the callback list after the quads and the text. Two scenes in one
    /// bucket keep the link-stamp order below the rank (registration order = FIFO), which the
    /// bare slot's own insertion carries here.
    #[inline]
    #[must_use]
    pub const fn callback(self, layer: DrawLayer) -> ZKey {
        ZKey(
            self.0
                | (1u64 << IS_REGION_SHIFT)
                | ((layer.index() as u64) << LAYER_SHIFT)
                | (128u64 << SUBLEVEL_SHIFT)
                | ((BatchRank::Callback as u64) << RANK_SHIFT),
        )
    }

    /// The raw packed value (the sort key).
    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Unpack this key back into its fields.
    #[inline]
    pub const fn parts(self) -> ZParts {
        unpack(self.0)
    }
}

/// The fields of a packed [`ZKey`], in significance order — the readable face of an opaque `u64`.
///
/// A draw-order key is a single number by design (one sort produces the whole render list), which
/// makes every question *about* an order — "why is this over that?", "which pairs would a re-rank
/// invert?" — unanswerable at a glance. [`unpack`] is that answer, and it is why
/// [`crate::script::ExtractedQuad::z`] can stay a bare `u64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZParts {
    /// The frame strata bucket id (0..=9, [`Strata::index`]).
    pub strata: u8,
    /// The frame level within the strata bucket.
    pub level: u16,
    /// The draw layer (0..=4, [`DrawLayer::index`]) — bucket-wide, above the frame.
    pub layer: u8,
    /// The batch rank: all textures of a `(strata, level, layer)` precede all its text, and all its
    /// text precedes its model scenes ([`BatchRank`]).
    pub rank: BatchRank,
    /// `rank == Text` — kept for the readers that only ask the old question.
    pub is_fontstring: bool,
    /// The owning frame's link-stamp — its position in the client's intrusive bucket list.
    pub insertion: u32,
    /// `false` for a frame's own slot, `true` for one of its regions.
    pub is_region: bool,
    /// The Era `textureSubLevel` knob; inert for 1.12 content (always 0 there).
    pub sub_level: i8,
    /// Declaration order within the owning frame's layer.
    pub decl: u16,
}

/// Unpack a raw [`ZKey`] (e.g. [`crate::script::ExtractedQuad::z`]) into its fields.
#[inline]
pub const fn unpack(raw: u64) -> ZParts {
    ZParts {
        strata: ((raw >> STRATUM_SHIFT) & 0xf) as u8,
        level: ((raw >> LEVEL_SHIFT) & 0xffff) as u16,
        layer: ((raw >> LAYER_SHIFT) & 0x7) as u8,
        rank: match (raw >> RANK_SHIFT) & 0x3 {
            0 => BatchRank::Quad,
            1 => BatchRank::Text,
            _ => BatchRank::Callback,
        },
        is_fontstring: (raw >> RANK_SHIFT) & 0x3 == 1,
        insertion: ((raw >> INSERTION_SHIFT) & ((1 << INSERTION_BITS) - 1)) as u32,
        is_region: (raw >> IS_REGION_SHIFT) & 1 == 1,
        sub_level: (((raw >> SUBLEVEL_SHIFT) & 0xff) as i16 - 128) as i8,
        decl: ((raw >> DECL_SHIFT) & ((1 << DECL_BITS) - 1)) as u16,
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Traversal — the visible render list in the client's painter order
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// What a [`ZKey`] entry points at: a frame's own draw slot, or one of its regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZTarget {
    /// The frame itself (backdrop/scissor slot) — sorts before its regions.
    Frame(FrameHandle),
    /// A texture or fontstring belonging to a frame.
    Region(RegionHandle),
}

/// The sorted draw list itself — shared, so a frame that changed nothing hands out the same one.
pub type DrawList = std::sync::Arc<[(ZTarget, ZKey)]>;

/// [`traversal`]'s memo: the last walk (arena order) and the sorted list built from it.
#[derive(Default, Debug)]
pub struct OrderCache(std::cell::RefCell<Option<OrderMemo>>);

/// The last walk, in arena order, and the sorted list built from it.
type OrderMemo = (Vec<(ZTarget, ZKey)>, DrawList);

/// Every `(target, key)` of the draw list, in ARENA order — the one walk both the fingerprint
/// and the rebuild are made of, so they cannot disagree.
fn walk(arena: &WidgetArena, mut f: impl FnMut(ZTarget, ZKey)) {
    for (fh, frame) in arena.iter_frames() {
        if !frame.effective_visible {
            continue;
        }
        let (strata, level, insertion) = (frame.strata, frame.level, frame.insertion_seq);
        f(ZTarget::Frame(fh), ZKey::frame(strata, level, insertion));
        for &rh in &frame.regions {
            let Some(region) = arena.region(rh) else {
                continue;
            };
            // `Region:SetParent(nil)` orphaned this leaf: unlinked from every draw layer, still
            // alive (see [`crate::widget::Region::detached`]). It draws nothing until re-parented.
            if region.detached {
                continue;
            }
            let key = ZKey::region(
                strata,
                level,
                insertion,
                region.draw_layer,
                region.sub_level,
                matches!(region.kind, RegionKind::FontString),
                region.decl_seq as u16,
            );
            f(ZTarget::Region(rh), key);
        }
    }
}

/// Produce the render list in the client's exact `0x765650` order.
///
/// Only **effective-visible** frames contribute: because `effective_visible` is maintained across
/// the whole subtree by the arena's propagation (`set_shown`/`set_parent`), filtering on the flag
/// is equivalent to the client's "recurse to child frames, a hidden mid-tree frame blocks its
/// subtree" (`0x76ae10`/`0x76ad50`) — a child of a hidden frame already carries
/// `effective_visible == false` and is skipped here. Each visible frame emits its own
/// [`ZTarget::Frame`] entry followed by one
/// [`ZTarget::Region`] per owned region; the returned vec is sorted ascending by [`ZKey`], which *is*
/// the total draw order (strata → frame level → **draw layer** → texture<fontstring → frame
/// link-stamp → is-region → sub-level → decl; the layer outranks the frame — see the `ZKey`
/// bit-layout note and decision 0884). Ties are impossible: distinct frames differ in link-stamp, a frame and its
/// regions differ in the is-region bit, and a frame's regions differ in the remaining fields.
///
/// Ordering only: this emits an entry for *every* region of a visible frame. Region-level
/// `Show`/`Hide` (the VisibleRegion bit, `region+0xc4`) is applied one layer up, where paint lives —
/// [`UiScript::extract`](crate::script::UiScript::extract) drops hidden regions before they become
/// quads. (The 1.12 region-draw cluster that would pin the *draw-time* skip remains unread; the
/// flag itself and its setter `0x77fcb0` are recorded.)
///
/// Cached against its own inputs: the list
/// is a pure function of every visible frame's `(strata, level, insertion)` and every attached
/// region's layer/sub-level/kind/decl — a walk over ~2 k frames and ~6 k regions, then a sort,
/// and it was rebuilt on every frame for the extract, the pointer and the edit box alike. The
/// walk stays (it is the fingerprint); the sort and the allocation happen only when the walk
/// hashes differently from the last one, which on a still frame it never does.
pub fn traversal(arena: &WidgetArena) -> DrawList {
    let mut cache = arena.order_cache.0.borrow_mut();
    // One walk into a scratch (the previous walk's buffer, reused), then an EXACT compare
    // against the last walk: equal inputs hand out the same list; a hash would have made a
    // wrong draw order a possibility, however remote, and cost a second walk on every miss.
    let (mut walked, last) = match cache.take() {
        Some((prev, list)) => {
            let mut scratch = Vec::with_capacity(prev.len());
            walk(arena, |t, k| scratch.push((t, k)));
            if scratch == prev {
                *cache = Some((prev, list.clone()));
                return list;
            }
            (scratch, Some(prev))
        }
        None => {
            let mut scratch = Vec::new();
            walk(arena, |t, k| scratch.push((t, k)));
            (scratch, None)
        }
    };
    drop(last);
    let mut out = walked.clone();
    out.sort_by_key(|&(_, k)| k);
    let list: DrawList = out.into();
    walked.shrink_to_fit();
    *cache = Some((walked, list.clone()));
    list
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Hit-testing — the mouse-focus capture walk
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Find the frame the cursor captures — the reference's flat hit-test index.
///
/// The sweep `0x7660d0` walks the mouse plane strata high→low and, within a stratum, the index from
/// 0 upward, ending the ENTIRE sweep on the first probe that returns non-zero. There is **no
/// fall-through**: `0x7663f4 call [edx+0x68]` is followed by `0x7663f7 mov eax,1`, so the handler's
/// return is destroyed before it is tested. A frame that takes the mouse and does nothing with the
/// click eats it — a `RegisterForClicks` miss, a disabled button and an unbound script all end the
/// event identically. (The one real fall-through in the engine is the WHEEL plane, which does gate
/// on a handler and continue; the engine has the idiom and deliberately does not use it here.)
///
/// **The index is FLAT** — sorted `(strata, frame level, insertion order)`, with no parent/child
/// nesting term at all, which is exactly [`traversal`]'s key. The tie is the part that matters and
/// the part we had backwards: `0x764aa0`'s comparison is a strict `ja`, so a frame arriving at a
/// level that already exists is **appended after** its equals, and the sweep from index 0 therefore
/// reaches the **earlier-linked** frame first. `SetFrameLevel` relinks the parent before recursing
/// into children, and effective-Show registers self (`0x76ae85`) before its children (`0x76aeb6`) —
/// so at equal levels **the parent wins over its own child**.
///
/// That is the whole reason `TargetFrame_OnLoad`'s `SetFrameLevel(textureFrame-1)` on the two bars
/// works: the bars land at exactly `TargetFrame`'s own level, and the tie goes to the frame that
/// registered first. Walking the draw order in reverse inverts it and hands the click to the
/// child, so right-clicking a unit frame opened nothing.
///
/// **The SIBLING case is the one that bites hardest, and it is why this is not a local fix**.
/// All children of one parent share `parent.level + 1`, so every sibling set is one
/// big tie, resolved purely by declaration order — and under the true law the FIRST-declared sibling
/// wins, not the last. A full-area mouse-enabled overlay declared at the top of a `<Frames>` list
/// therefore swallows its whole window. Five of our own windows shipped exactly that (the
/// `*WheelCatcher` `<Button>`s), correct only under the inverted order and now removed;
/// `SkillFrame.xml` had already recorded one shipped bug from the same shape. Note that
/// `<Button>` is mouse-enabled by its *constructor*, so such an overlay needs no
/// `enableMouse` to compete — the wheel is a separate index and needs no mouse hit target at all.
///
/// Drawing is unaffected and stays later-on-top: this reordering is the HIT sweep's alone.
///
/// Pure over its inputs: `sorted` is draw-order-**ascending** (as [`traversal`] returns), and
/// `hits(frame)` reports whether that frame is a capture candidate at the cursor. Visibility is
/// handled upstream. Regions never capture.
pub fn hit_test<F: Fn(FrameHandle) -> bool>(
    sorted: &[(ZTarget, ZKey)],
    hits: F,
) -> Option<FrameHandle> {
    // Everything above the insertion field: one (strata, level) plane of the reference's index.
    let plane = |k: ZKey| k.0 >> LEVEL_SHIFT;
    let mut end = sorted.len();
    while end > 0 {
        let p = plane(sorted[end - 1].1);
        let mut start = end;
        while start > 0 && plane(sorted[start - 1].1) == p {
            start -= 1;
        }
        // Planes descend; WITHIN a plane the earlier-linked frame is probed first.
        for (target, _) in &sorted[start..end] {
            if let ZTarget::Frame(fh) = *target {
                if hits(fh) {
                    return Some(fh);
                }
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
        // BLIZZARD (Era) is above TOOLTIP.
        assert!(ZKey::frame(Strata::Blizzard, 0, 0) > ZKey::frame(Strata::Tooltip, u16::MAX, 0));
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
        // The layer is the top term below (strata, level) — it dominates everything under it.
        assert!(
            mk(DrawLayer::Border, i8::MIN, false, 0) > mk(DrawLayer::Background, 127, true, 4095)
        );
        // Within a layer, KIND is next: all textures precede all fontstrings — and it now outranks
        // sub-level (the layer batch's two sub-arrays, quads drained before text).
        assert!(
            mk(DrawLayer::Artwork, i8::MIN, true, 0) > mk(DrawLayer::Artwork, 127, false, 4095)
        );
        // Below kind, declaration order decides within one frame's layer.
        assert!(mk(DrawLayer::Artwork, 0, false, 2) > mk(DrawLayer::Artwork, 0, false, 1));
        // Sub-level still orders below kind — inert on 1.12 (nothing sets it), kept for Era.
        assert!(mk(DrawLayer::Artwork, -1, false, 0) < mk(DrawLayer::Artwork, 0, false, 0));
    }

    /// The law 0884 **replaced**, kept as a test so it cannot come back: a frame's regions are not
    /// glued behind it. The draw layer is bucket-wide, so at one `(strata, level)` every frame's
    /// BACKGROUND draws before any frame's OVERLAY — frame A's OVERLAY sorts *after* frame B's
    /// BACKGROUND even though A linked first. That interleave is what puts the mirror timer's
    /// ARTWORK fill under its own parent frame's OVERLAY border and caption.
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

        // Kind spans frames: ALL textures of a (strata, level, layer) precede ALL its fontstrings.
        let a_text = ZKey::region(Strata::Medium, 0, 10, DrawLayer::Artwork, 0, true, 0);
        assert!(
            b_art < a_text,
            "a later frame's texture still precedes an earlier frame's fontstring"
        );
        // …and ALL its font strings precede a model's scene, whatever the link stamps: the scene
        // drains from the layer batch's callback list, after its quads and its text (1995). The
        // world map's arrow (a Model linked FIRST under WorldMapFrame) over the zone overlays and
        // the area label (a later-linked ARTWORK texture and font string) is exactly this pair.
        let earlier_model = ZKey::frame(Strata::Medium, 0, 9).callback(DrawLayer::Artwork);
        let later_text = ZKey::region(Strata::Medium, 0, 11, DrawLayer::Artwork, 0, true, 0);
        assert!(
            later_text < earlier_model,
            "an earlier-linked model's scene still draws after a later frame's ARTWORK text"
        );
        // The rank sits BELOW the layer: an OVERLAY quad still covers an ARTWORK scene.
        let b_overlay = ZKey::region(Strata::Medium, 0, 11, DrawLayer::Overlay, 0, false, 0);
        assert!(
            earlier_model < b_overlay,
            "the layer outranks the batch rank"
        );
        // Two scenes in one bucket: registration order, which the link stamp carries.
        let later_model = ZKey::frame(Strata::Medium, 0, 12).callback(DrawLayer::Artwork);
        assert!(earlier_model < later_model);
        assert_eq!(unpack(earlier_model.raw()).rank, BatchRank::Callback);

        // A frame's own slot is benilla's backdrop/scissor hook: it sits BELOW the layer key, so
        // it precedes only its own regions, starting with its BACKGROUND.
        let a_frame = ZKey::frame(Strata::Medium, 0, 10);
        let a_bg = ZKey::region(Strata::Medium, 0, 10, DrawLayer::Background, 0, false, 0);
        assert!(a_frame < a_bg);
    }

    // ── Traversal over a synthetic tree ────────────────────────────────────────────────────────

    use crate::widget::{FrameKind, WidgetArena};

    #[test]
    fn traversal_snapshot_over_a_small_tree() {
        let mut a = WidgetArena::new();

        // A DIALOG-strata parent with a texture; a MEDIUM child (lower strata ⇒ draws *before* its
        // parent, proving the order is flat, not hierarchical); and a hidden frame that must not
        // appear at all.
        let dialog = a.create(FrameKind::Frame, Some("Dialog".into()), None);
        a.set_frame_strata(dialog, Strata::Dialog);
        let dlg_bg = a
            .create_region(dialog, RegionKind::Texture, DrawLayer::Background, 0)
            .unwrap();
        let dlg_text = a
            .create_region(dialog, RegionKind::FontString, DrawLayer::Artwork, 0)
            .unwrap();

        // set_frame_strata forced the child to DIALOG too (subtree force), so make the child a
        // top-level MEDIUM frame instead to exercise cross-strata draw order.
        let medium = a.create(FrameKind::Frame, None, None); // MEDIUM, level 0
        let med_art = a
            .create_region(medium, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap();

        let hidden = a.create(FrameKind::Frame, None, None);
        let _hidden_rgn = a
            .create_region(hidden, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap();
        a.set_shown(hidden, false);

        let list: Vec<ZTarget> = traversal(&a).iter().map(|&(t, _)| t).collect();

        // Expected painter order: MEDIUM frame + its region first (lower strata), then the DIALOG
        // frame followed by its two regions (Background texture before Artwork fontstring). The
        // hidden frame and its region are absent.
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

    /// The client's bucket re-add on show (`0x76ae10`):
    /// within one (strata, level) bucket, a frame shown LATER draws over one declared later but
    /// never hidden — the exact minimap case (MiniMapTrackingFrame is declared before
    /// MinimapBackdrop, hidden at load, and its runtime Show must lift it over the backdrop's
    /// ring art, as the director's reference A/B shows). A strata/level change on
    /// a visible frame re-buckets to the tail the same way; on a hidden frame it does not bump
    /// (the client's remove/add is visible-gated — it appends on the next show regardless).
    #[test]
    fn showing_a_frame_moves_it_to_its_buckets_tail() {
        let mut a = WidgetArena::new();
        let tracking = a.create(FrameKind::Frame, Some("Tracking".into()), None);
        let backdrop = a.create(FrameKind::Frame, Some("Backdrop".into()), None);

        let order =
            |a: &WidgetArena| -> Vec<ZTarget> { traversal(a).iter().map(|&(t, _)| t).collect() };
        // Declaration order while both start shown: tracking before backdrop.
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(tracking), ZTarget::Frame(backdrop)]
        );

        // Hide-then-show (the XML's hidden="true" + the runtime Show()): tracking re-enters its
        // bucket at the tail — above the backdrop.
        a.set_shown(tracking, false);
        a.set_shown(tracking, true);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(backdrop), ZTarget::Frame(tracking)]
        );

        // A no-op Show (already visible) is NOT a transition — no re-add, order stays.
        a.set_shown(backdrop, true);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(backdrop), ZTarget::Frame(tracking)]
        );

        // A visible frame's level round-trip re-buckets it to the tail both times — the
        // remove/add of set_frame_level. (backdrop: level 0 → 1 → 0, ends level 0 again but now
        // draws over tracking.)
        a.set_frame_level(backdrop, 1, false);
        a.set_frame_level(backdrop, 0, false);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(tracking), ZTarget::Frame(backdrop)]
        );

        // The same round-trip while HIDDEN doesn't bump: after the show it sits at the tail
        // because of the show itself, and a hidden frame is in no bucket to move.
        a.set_shown(tracking, false);
        a.set_frame_level(tracking, 1, false);
        a.set_frame_level(tracking, 0, false);
        a.set_shown(tracking, true);
        assert_eq!(
            order(&a),
            vec![ZTarget::Frame(backdrop), ZTarget::Frame(tracking)]
        );
    }

    // ── Hit-testing (pure core) ─────────────────────────────────────────────────────────────────

    #[test]
    fn hit_test_returns_topmost_matching_frame_and_skips_regions() {
        let mut a = WidgetArena::new();
        // Two overlapping frames; `top` (DIALOG) draws above `bottom` (MEDIUM). A region belongs to
        // `top` and would sort *after* the frame in draw order — it must never be returned.
        let bottom = a.create(FrameKind::Frame, None, None);
        let top = a.create(FrameKind::Frame, None, None);
        a.set_frame_strata(top, Strata::Dialog);
        let _rgn = a
            .create_region(top, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap();

        let sorted = traversal(&a);
        // Both frames are candidates ⇒ the topmost-drawn (`top`) wins.
        assert_eq!(hit_test(&sorted, |_| true), Some(top));
        // Only `bottom` is a candidate ⇒ it wins even though `top` draws above it (top is transparent
        // to hits, e.g. mouse-disabled).
        assert_eq!(hit_test(&sorted, |fh| fh == bottom), Some(bottom));
        // No candidate ⇒ no capture.
        assert_eq!(hit_test(&sorted, |_| false), None);
    }
}
