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
//! The frame term is the client's live intrusive-list position: a frame relinks at its bucket's
//! tail when shown and when a shown frame's strata or level changes, and the later link draws on
//! top. [`crate::widget::WidgetArena`] keeps a counter bumped at those moments, which orders the
//! same within a bucket. The is-region bit is benilla's: the client has no frame drawable, and the
//! frame's own slot (backdrop, scissor) must precede its regions.
//!
//! Deviation: textures in one layer keep link order where the client sorts them by texture handle
//! (`0x7731a0`, comparator `0x7731c0`), because benilla allocates its handles differently; no
//! content may depend on the order of two overlapping textures in one layer.

use crate::widget::{FrameHandle, RegionHandle, RegionKind, WidgetArena};

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
    /// A later client's stratum above `Tooltip`, not a 1.12 one.
    Blizzard = 9,
}

impl Strata {
    /// Every stratum in draw order.
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
// Most significant first; one ascending sort gives the render list.
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
// The sub-level is a later client's ordering within a layer, not 1.12's, which has none
// (`0x76a860` takes only region and layer and inserts at the head); stock content never sets it.

const STRATUM_SHIFT: u32 = 60;
const LEVEL_SHIFT: u32 = 44;
const LAYER_SHIFT: u32 = 41;
const RANK_SHIFT: u32 = 39;
const INSERTION_SHIFT: u32 = 21;
const IS_REGION_SHIFT: u32 = 20;
const SUBLEVEL_SHIFT: u32 = 12;
const DECL_SHIFT: u32 = 0;

/// The link-stamp width; the arena renumbers its stamps at the cap.
pub(crate) const INSERTION_BITS: u32 = 18;
const DECL_BITS: u32 = 12;

/// Which of a layer batch's arrays an entry drains from, in `0x76fb00`'s order: quads at `+0x10`
/// (a texture appends at `0x7706e0`), a `CGxStringBatch` at `+0x18` (a font string at `0x772e50`)
/// and a render-callback list at `+0x1c` (ctor `0x772e80`). So in one `(strata, level, layer)`
/// every texture precedes every font string, and every font string every model scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BatchRank {
    Quad = 0,
    Text = 1,
    /// A `Model` frame's scene, registered for ARTWORK only (`0x76d17f`).
    Callback = 2,
}

/// The packed draw-order key; its `Ord` is the client's draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZKey(u64);

impl ZKey {
    /// A frame's own entry; its region fields are zero, so it sorts before its regions.
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
            insertion < (1 << INSERTION_BITS),
            "frame insertion {insertion} exceeds {INSERTION_BITS} bits"
        );
        debug_assert!(
            u32::from(decl) < (1 << DECL_BITS),
            "region decl seq {decl} exceeds {DECL_BITS} bits"
        );
        // Biased into 0..=255 so a higher sub-level sorts later.
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

    /// The key for a frame's own content, text with no region: its frame slot moved into its
    /// region band at `layer` with the text rank. The content is a ScrollingMessageFrame's lines,
    /// which the client draws as the frame's own font strings (ARTWORK with no `<Layer>`), so the
    /// frame's BACKGROUND art stays behind them.
    #[inline]
    #[must_use]
    pub const fn content(self, layer: DrawLayer) -> ZKey {
        ZKey(
            self.0
                | (1u64 << IS_REGION_SHIFT)
                | ((layer.index() as u64) << LAYER_SHIFT)
                // Sub-level 0, biased as in `region`.
                | (128u64 << SUBLEVEL_SHIFT)
                | ((BatchRank::Text as u64) << RANK_SHIFT),
        )
    }

    /// The key for a `Model` frame's scene: its frame slot moved into its region band at `layer`
    /// with the callback rank, after every texture and font string of the layer, as `0x76d160`
    /// registers the scene into the layer batch whose callbacks `0x76fb00` drains last.
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

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn parts(self) -> ZParts {
        unpack(self.0)
    }
}

/// A [`ZKey`]'s fields, most significant first.
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
}

/// A raw [`ZKey`]'s fields, as [`crate::script::ExtractedQuad::z`] stores the key.
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

// ── Traversal: the visible render list in painter order ──────────────────────────────────────

/// What a [`ZKey`] entry points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ZTarget {
    /// The frame's own slot (backdrop, scissor).
    Frame(FrameHandle),
    Region(RegionHandle),
}

/// The sorted draw list, shared so an unchanged frame hands out the same one.
pub type DrawList = std::sync::Arc<[(ZTarget, ZKey)]>;

/// [`traversal`]'s memo: the last walk, in arena order, and the sorted list built from it.
#[derive(Default, Debug)]
pub struct OrderCache(std::cell::RefCell<Option<OrderMemo>>);

type OrderMemo = (Vec<(ZTarget, ZKey)>, DrawList);

/// Every `(target, key)` of the draw list, in arena order.
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
            // A region orphaned by `SetParent(nil)` is in no draw layer until re-parented.
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

/// The render list in the client's `0x765650` order, sorted by [`ZKey`], with no two keys tied.
/// Only effective-visible frames contribute, as the client skips a hidden frame's whole subtree
/// (`0x76ae10`, `0x76ad50`). Every region of such a frame is listed; hidden ones (`region+0xc4`,
/// set by `0x77fcb0`) are dropped by [`UiScript::extract`](crate::script::UiScript::extract), and
/// where the client skips them at draw time is untraced. An unchanged walk returns the last list.
pub fn traversal(arena: &WidgetArena) -> DrawList {
    let mut cache = arena.order_cache.0.borrow_mut();
    // An exact compare with the last walk, not a hash: a collision would draw in a wrong order.
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
/// `sorted` is [`traversal`]'s ascending list and `hits` says whether a frame is a candidate at the
/// cursor; regions never capture.
pub fn hit_test<F: Fn(FrameHandle) -> bool>(
    sorted: &[(ZTarget, ZKey)],
    hits: F,
) -> Option<FrameHandle> {
    // The key's strata and level: one plane of the client's index.
    let plane = |k: ZKey| k.0 >> LEVEL_SHIFT;
    let mut end = sorted.len();
    while end > 0 {
        let p = plane(sorted[end - 1].1);
        let mut start = end;
        while start > 0 && plane(sorted[start - 1].1) == p {
            start -= 1;
        }
        // Planes descend; within one, the earlier-linked frame is probed first.
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
        // BLIZZARD is above TOOLTIP.
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

        let list: Vec<ZTarget> = traversal(&a).iter().map(|&(t, _)| t).collect();

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
            |a: &WidgetArena| -> Vec<ZTarget> { traversal(a).iter().map(|&(t, _)| t).collect() };
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

    // ── Hit-testing (pure core) ─────────────────────────────────────────────────────────────────

    #[test]
    fn hit_test_returns_topmost_matching_frame_and_skips_regions() {
        let mut a = WidgetArena::new();
        // `top` (DIALOG) draws above `bottom` (MEDIUM); `top`'s region must never be returned.
        let bottom = a.create(FrameKind::Frame, None, None);
        let top = a.create(FrameKind::Frame, None, None);
        a.set_frame_strata(top, Strata::Dialog);
        let _rgn = a
            .create_region(top, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap();

        let sorted = traversal(&a);
        assert_eq!(hit_test(&sorted, |_| true), Some(top));
        // A `top` that does not take the mouse lets `bottom` win.
        assert_eq!(hit_test(&sorted, |fh| fh == bottom), Some(bottom));
        assert_eq!(hit_test(&sorted, |_| false), None);
    }
}
