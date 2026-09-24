//! The ScrollFrame clip, shared by `extract` and the pointer hit-test so drawing and hitting agree.

use std::collections::HashMap;

use crate::layout::Rect;
use crate::widget::{FrameKind, KindState};

use super::model::Model;
use super::FrameHandle;

/// Scroll child → its ScrollFrame's resolved rect, for every live ScrollFrame with a rect and a
/// live child. Rebuilt per call and never cached, so no scroll change needs invalidating.
pub(super) fn scroll_clip_sources(model: &Model) -> HashMap<FrameHandle, Rect> {
    let mut sources = HashMap::new();
    for (h, frame) in model.arena.iter_frames() {
        if frame.kind != FrameKind::ScrollFrame {
            continue;
        }
        let KindState::Scroll(state) = &frame.kind_state else {
            continue;
        };
        let Some(child) = state.child else { continue };
        if model.arena.frame(child).is_none() {
            continue; // stale child handle
        }
        if let Some(&rect) = model.resolved.get(&h) {
            sources.insert(child, rect);
        }
    }
    sources
}

/// The clip `h` draws and hits within: the intersection of every ScrollFrame rect whose scroll
/// child is `h` or an ancestor of it, so nested ScrollFrames intersect; `None` is unclipped.
pub(super) fn effective_clip(
    model: &Model,
    sources: &HashMap<FrameHandle, Rect>,
    mut h: FrameHandle,
) -> Option<Rect> {
    let mut clip: Option<Rect> = None;
    loop {
        if let Some(&r) = sources.get(&h) {
            clip = Some(match clip {
                Some(c) => intersect_rect(c, r),
                None => r,
            });
        }
        match model.arena.frame(h).and_then(|f| f.parent) {
            Some(p) => h = p,
            None => break,
        }
    }
    clip
}

/// Rect intersection, y-up. Disjoint rects give an inverted rect, which the pointer never hits
/// and the app's `clip_quad` draws nothing in.
pub(super) fn intersect_rect(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.bottom.max(b.bottom),
        a.left.max(b.left),
        a.top.min(b.top),
        a.right.min(b.right),
    )
}
