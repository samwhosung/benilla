//! The ScrollFrame clips: the draw clip of a scroll child's subtree, which `extract` applies, and
//! the hit clip of a frame in it, which the pointer's probe applies. The two differ in the
//! reference, and so here.

use std::collections::HashMap;

use crate::layout::Rect;
use crate::order::{DrawEntry, ZTarget};

use super::model::Model;
use super::FrameHandle;

/// A frame's mouse rect (`0x76b580`, `[frame+0xd8]`): its resolved rect shrunk by its `[left,
/// right, top, bottom]` hit-rect insets times its effective scale (y-up), the insets being in the
/// frame's own units; an over-shrunk rect collapses to empty rather than inverting.
pub(super) fn hit_rect(model: &Model, fh: FrameHandle, r: Rect) -> Rect {
    let s = super::object::eff_scale(model, fh);
    let [left, right, top, bottom] = model.arena.hit_rect_insets(fh).map(|i| i * s);
    Rect {
        left: r.left + left,
        right: (r.right - right).max(r.left + left),
        top: r.top - top,
        bottom: (r.bottom + bottom).min(r.top - top),
    }
}

/// The clip a draw entry under a ScrollFrame takes, per [`DrawEntry::scope`]: the callback
/// `0x787220` narrows the viewport to the ScrollFrame's rect (`[+0xd8]`, `0x76b220`) intersected
/// with the viewport it was called in (`0x7873f5`–`0x787441`), so nested ScrollFrames intersect.
/// It draws nothing for a ScrollFrame whose rect has not resolved (`0x787253`), nor for a clip
/// without both spans strictly positive (`0x78750e`–`0x787526`), as nested disjoint rects give.
pub(super) struct DrawClips<'a> {
    model: &'a Model,
    /// Each frame's own scope, for the frames drawn under one; the rest are under none.
    scope_of: HashMap<FrameHandle, FrameHandle>,
    /// A ScrollFrame's callback clip, `None` when its callback draws nothing.
    memo: HashMap<FrameHandle, Option<Rect>>,
}

impl<'a> DrawClips<'a> {
    pub(super) fn new(model: &'a Model, list: &[DrawEntry]) -> DrawClips<'a> {
        let scope_of = list
            .iter()
            .filter_map(|e| match (e.target, e.scope) {
                (ZTarget::Frame(fh), Some(scope)) => Some((fh, scope)),
                _ => None,
            })
            .collect();
        DrawClips {
            model,
            scope_of,
            memo: HashMap::new(),
        }
    }

    /// The clip of an entry under `scope`: `Some(None)` unclipped, `Some(Some(rect))` clipped, and
    /// `None` when the entry is not drawn.
    pub(super) fn clip(&mut self, scope: Option<FrameHandle>) -> Option<Option<Rect>> {
        match scope {
            None => Some(None),
            Some(sf) => self.callback_clip(sf).map(Some),
        }
    }

    fn callback_clip(&mut self, sf: FrameHandle) -> Option<Rect> {
        if let Some(&c) = self.memo.get(&sf) {
            return c;
        }
        let own = self
            .model
            .resolved
            .get(&sf)
            .map(|&r| hit_rect(self.model, sf, r));
        let outer = self.scope_of.get(&sf).copied();
        let clip = match (own, outer) {
            (None, _) => None,
            (Some(r), None) => Some(r),
            (Some(r), Some(o)) => self.callback_clip(o).map(|oc| intersect_rect(oc, r)),
        }
        .filter(|c| c.right > c.left && c.top > c.bottom);
        self.memo.insert(sf, clip);
        clip
    }
}

/// The hit clip of `h`, as the probe `0x76b020` reads it: none for a frame outside every scroll
/// child's subtree (`0x76b041`); otherwise the mouse rect of its first ancestor without the scroll
/// flag (`0x76b047`–`0x76b069`), which the point must also be in. `None` is a miss whatever the
/// point: the parent chain ran out with every ancestor flagged (`0x76b063`). The ancestor's
/// `[+0xd8]` is read whatever its valid bit (`0x76b069`), so one that never resolved gives the
/// ctor's zero rect (`0x7690f6`–`0x769108`). One whose resolve failed after a good one keeps its
/// last rect in the client, which the layout does not keep for a frame: zero here too.
pub(super) fn hit_clip(model: &Model, h: FrameHandle) -> Option<Option<Rect>> {
    let f = model.arena.frame(h)?;
    if !f.scroll_member {
        return Some(None);
    }
    let mut cur = f.parent;
    while let Some(p) = cur {
        let pf = model.arena.frame(p)?;
        if !pf.scroll_member {
            let rect = model
                .resolved
                .get(&p)
                .map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |&r| hit_rect(model, p, r));
            return Some(Some(rect));
        }
        cur = pf.parent;
    }
    None
}

/// Rect intersection, y-up. Disjoint rects give an inverted rect, whose spans the caller tests.
pub(super) fn intersect_rect(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.bottom.max(b.bottom),
        a.left.max(b.left),
        a.top.min(b.top),
        a.right.min(b.right),
    )
}
