//! [`WidgetArena`]'s mutators: propagation down the subtree, reparenting and the frame flags.

use crate::order::Strata;

use super::{FrameHandle, RegionHandle, WidgetArena, SCALE_EPS};

impl WidgetArena {
    // ── Visibility ───────────────────────────────────────────────────────────────────────────────

    /// Set `shown` and propagate effective visibility down the subtree (show `0x76ae10`, hide
    /// `0x76ad50`), returning the frames that changed, a node before its descendants, for the
    /// caller to fire `OnShow`/`OnHide`. The reference notifies post-order (`0x76aef5`) and
    /// re-reads live links, so a frame a handler hides mid-cascade fails its gate (`0x76ae1d`) and
    /// is never notified; this list is a snapshot that still holds it.
    pub fn set_shown(&mut self, h: FrameHandle, shown: bool) -> Vec<FrameHandle> {
        let mut changed = Vec::new();
        match self.frame_mut(h) {
            Some(f) if f.shown != shown => f.shown = shown,
            _ => return changed,
        }
        let parent_visible = self.parent_visible(h);
        self.propagate_visible(h, parent_visible, &mut changed);
        changed
    }

    fn propagate_visible(
        &mut self,
        h: FrameHandle,
        parent_visible: bool,
        changed: &mut Vec<FrameHandle>,
    ) {
        let (shown, old_ev, children) = {
            let f = self.frame(h).expect("live node in propagation");
            (f.shown, f.effective_visible, f.children.clone())
        };
        let new_ev = shown && parent_visible;
        if new_ev == old_ev {
            // No change here means none below: prune, as the client's recursion does.
            return;
        }
        self.frame_mut(h).unwrap().effective_visible = new_ev;
        if new_ev {
            // Re-added at its bucket's tail, as the client does, and each descendant after it.
            self.resequence_to_tail(h);
        }
        changed.push(h);
        for c in children {
            self.propagate_visible(c, new_ev, changed);
        }
    }

    // ── Strata ───────────────────────────────────────────────────────────────────────────────────

    /// Force `h` and its whole subtree to `strata` (`0x76a470`), moving each visible frame to the
    /// tail of its new bucket.
    pub fn set_frame_strata(&mut self, h: FrameHandle, strata: Strata) {
        let (visible, children) = match self.frame_mut(h) {
            Some(f) => {
                if f.strata == strata {
                    return;
                }
                f.strata = strata;
                (f.effective_visible, f.children.clone())
            }
            None => return,
        };
        if visible {
            self.resequence_to_tail(h);
        }
        for c in children {
            self.set_frame_strata(c, strata);
        }
    }

    // ── Level ────────────────────────────────────────────────────────────────────────────────────

    /// Set `h`'s level (`0x76a4f0`); with `propagate`, same-strata children shift by the same
    /// delta. A same-value call returns before relinking (`0x76a509`).
    pub fn set_frame_level(&mut self, h: FrameHandle, level: u16, propagate: bool) {
        let (delta, strata, children) = match self.frame_mut(h) {
            Some(f) => {
                if f.level == level {
                    return;
                }
                let delta = i32::from(level) - i32::from(f.level);
                f.level = level;
                (delta, f.strata, f.children.clone())
            }
            None => return,
        };
        // Only a visible frame relinks (`0x76a541`); a hidden one is in no bucket.
        if self.frame(h).is_some_and(|f| f.effective_visible) {
            self.resequence_to_tail(h);
        }
        if !propagate {
            return;
        }
        for c in children {
            let child_new = match self.frame(c) {
                Some(cf) if cf.strata == strata => {
                    (i32::from(cf.level) + delta).clamp(0, i32::from(u16::MAX)) as u16
                }
                _ => continue, // cross-strata or stale: untouched
            };
            self.set_frame_level(c, child_new, true);
        }
    }

    /// Level compaction (`0x764eb0`), run by `CSimpleTop::Raise` (`0x7650f0`) before it sets the
    /// raised frame's level to the returned count: renumber the occupied levels of `strata` in
    /// order, hidden frames included (the scan tests only strata and level, `0x764f18`/`0x764f20`),
    /// without propagating (`0x764faf`). `level` is written directly, so `insertion_seq` survives
    /// where the reference re-levels through `0x76a4f0` (`0x764fb6`) and relinks visible frames.
    ///
    /// Deviation: levels pack contiguously, because the reference's resuming cursor
    /// (`0x764fd3`/`0x764fd8`) leaves a hole past a wide gap ({0, 4, 6} at count 7 gives {0, 1, 3}
    /// at count 4) that no draw or hit order can see.
    pub fn compact_levels(&mut self, strata: Strata) -> u16 {
        let mut occupied: Vec<u16> = self
            .iter_frames()
            .filter(|(_, f)| f.strata == strata)
            .map(|(_, f)| f.level)
            .collect();
        occupied.sort_unstable();
        occupied.dedup();
        let renumber: Vec<(FrameHandle, u16)> = self
            .iter_frames()
            .filter(|(_, f)| f.strata == strata)
            .filter_map(|(h, f)| {
                let idx = occupied.binary_search(&f.level).ok()? as u16;
                (idx != f.level).then_some((h, idx))
            })
            .collect();
        for (h, level) in renumber {
            if let Some(f) = self.frame_mut(h) {
                f.level = level;
            }
        }
        occupied.len() as u16
    }

    // ── Toplevel ─────────────────────────────────────────────────────────────────────────────────

    /// `SetToplevel` (`0x775440`) via the bit-setter `0x76a3c0`: a flag write that raises nothing.
    pub fn set_toplevel(&mut self, h: FrameHandle, toplevel: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.toplevel = toplevel;
        }
    }

    pub fn is_toplevel(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.toplevel)
    }

    // ── Scale ────────────────────────────────────────────────────────────────────────────────────

    /// Set `h`'s own scale and recompute `effective_scale = parent's * own` down the subtree
    /// (`0x76ac90`), stopping at a frame whose value moves less than [`SCALE_EPS`].
    pub fn set_scale(&mut self, h: FrameHandle, scale: f32) {
        if let Some(f) = self.frame_mut(h) {
            f.scale = scale;
        } else {
            return;
        }
        let parent_scale = self.parent_effective_scale(h);
        self.propagate_scale(h, parent_scale);
    }

    fn propagate_scale(&mut self, h: FrameHandle, parent_scale: f32) {
        let (own, ignore, old_eff, children) = {
            let f = self.frame(h).expect("live node in propagation");
            (
                f.scale,
                f.ignore_parent_scale,
                f.effective_scale,
                f.children.clone(),
            )
        };
        let new_eff = if ignore { own } else { parent_scale * own };
        if (f64::from(new_eff) - f64::from(old_eff)).abs() < SCALE_EPS {
            return;
        }
        self.frame_mut(h).unwrap().effective_scale = new_eff;
        for c in children {
            self.propagate_scale(c, new_eff);
        }
    }

    // ── Alpha ────────────────────────────────────────────────────────────────────────────────────

    /// `SetAlpha` (`0x76a690`): write the raw value to `h` and every descendant frame (`+0xc8`),
    /// with no draw-time product, so a child's own later `SetAlpha` holds until an ancestor's next.
    pub fn set_alpha(&mut self, h: FrameHandle, alpha: f32) {
        let Some(f) = self.frame_mut(h) else { return };
        f.alpha = alpha;
        f.effective_alpha = alpha;
        let children = self.frame(h).expect("just wrote it").children.clone();
        for c in children {
            self.set_alpha(c, alpha);
        }
    }

    // ── Reparenting ──────────────────────────────────────────────────────────────────────────────

    /// Phase 1 of `SetParent` (`0x76ab10`): the guards and the hide half. `None` when the parent
    /// is unchanged (`0x76ab20` skips everything), the frame is dead, or the move would cycle (the
    /// Lua binding raises first, `0x7a177f`); otherwise the frames that lost effective visibility,
    /// in pre-order, whose `OnHide` runs under the old parent (`0x76ab41`).
    ///
    /// Deviation: the frame is still in the old parent's child list during `OnHide`, unlike the
    /// reference, because a handler that itself reparents must not find the lists half-spliced.
    pub fn reparent_begin(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
    ) -> Option<Vec<FrameHandle>> {
        self.frame(h)?;
        let new_parent = new_parent.filter(|&p| self.frame(p).is_some());
        if let Some(np) = new_parent {
            if np == h || self.is_ancestor(h, np) {
                return None;
            }
        }
        if self.frame(h).unwrap().parent == new_parent {
            return None;
        }
        let mut hidden = Vec::new();
        if self.frame(h).unwrap().effective_visible {
            self.propagate_visible(h, false, &mut hidden);
        }
        Some(hidden)
    }

    /// Phase 2 of `SetParent` (`0x76ab10`): relink, strata := the parent's (`0x76ab5a`, MEDIUM for
    /// none), level := the parent's + 1 without propagating (`0x76ab65`, 0 for none), scale
    /// re-inherited, then the show half, returning the frames that became visible; alpha is
    /// untouched. `h`'s own children keep their levels, so one can end up below `h`.
    /// `was_visible` is whether phase 1 hid anything (the reference's `ebx` at `0x76ab2b`); the
    /// show half runs only then (`0x76abfd`), so a hidden frame moved under a visible parent keeps
    /// a stale effective visibility until something shows it.
    pub fn reparent_finish(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
        was_visible: bool,
    ) -> Vec<FrameHandle> {
        let mut shown = Vec::new();
        if self.frame(h).is_none() {
            return shown; // died inside an OnHide: nothing to move
        }
        let new_parent = new_parent.filter(|&p| self.frame(p).is_some());
        // Relink from the current parent: an `OnHide` may have reparented it, and the reference's
        // unconditional `+0x9c` store (`0x76ab49`) lets the outer call win.
        let old_parent = self.frame(h).unwrap().parent;
        if let Some(op) = old_parent {
            if let Some(of) = self.frame_mut(op) {
                of.children.retain(|&c| c != h);
            }
        }
        self.frame_mut(h).unwrap().parent = new_parent;
        if let Some(np) = new_parent {
            self.frame_mut(np).unwrap().children.push(h);
        }

        let (pstrata, plevel) = match new_parent {
            Some(np) => {
                let pf = self.frame(np).expect("live new parent");
                (pf.strata, pf.level.saturating_add(1))
            }
            // `SetParent(nil)` resets to MEDIUM and level 0 (`0x76aba3`/`0x76abac`).
            None => (Strata::default(), 0),
        };
        self.set_frame_strata(h, pstrata);
        self.set_frame_level(h, plevel, false);
        let ps = self.parent_effective_scale(h);
        self.propagate_scale(h, ps);

        if was_visible && self.parent_visible(h) {
            self.propagate_visible(h, true, &mut shown);
        }
        shown
    }

    /// Both reparent phases in one call, hide changes first, for a caller with no `OnHide` to fire.
    pub fn set_parent(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
    ) -> Vec<FrameHandle> {
        let Some(hidden) = self.reparent_begin(h, new_parent) else {
            return Vec::new();
        };
        let was_visible = !hidden.is_empty();
        let shown = self.reparent_finish(h, new_parent, was_visible);
        [hidden, shown].concat()
    }

    /// `Region:SetParent` for a texture or font string; `None` detaches. The client re-links in
    /// full (`0x7733d0` → `0x77fd10`): out of the old parent's layer (`0x77fc60`) and list, into
    /// the new parent's list (`0x76a750`) and layer (`0x77fcb0`), its layer kept, so the region
    /// takes the tail `decl_seq`. Readers resolve through `owner`, so nothing propagates. A
    /// same-owner call is a no-op, as in the reference (`0x77fd33`).
    pub fn set_region_owner(&mut self, rh: RegionHandle, new_owner: Option<FrameHandle>) -> bool {
        let Some(region) = self.region(rh) else {
            return false;
        };
        let (old_owner, was_detached) = (region.owner, region.detached);
        // No cycle check: the client's (`0x7a177f`) looks for the receiver in the new parent's
        // frame chain, and a region is never a frame.
        let new_owner = new_owner.filter(|&f| self.frame(f).is_some());
        match new_owner {
            Some(f) if f == old_owner && !was_detached => return false,
            None if was_detached => return false,
            _ => {}
        }
        let Some(new_owner) = new_owner else {
            if let Some(r) = self.region_mut(rh) {
                r.detached = true;
            }
            return true;
        };
        if let Some(of) = self.frame_mut(old_owner) {
            of.regions.retain(|&r| r != rh);
        }
        let decl_seq = {
            let f = self.frame_mut(new_owner).expect("live new owner");
            let d = f.next_decl;
            f.next_decl += 1;
            d
        };
        if let Some(r) = self.region_mut(rh) {
            r.owner = new_owner;
            r.decl_seq = decl_seq;
            r.detached = false;
        }
        self.frame_mut(new_owner)
            .expect("live new owner")
            .regions
            .push(rh);
        true
    }

    // ── Frame flags ──────────────────────────────────────────────────────────────────────────────

    /// `EnableMouse`.
    pub fn set_mouse_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.mouse_enabled = enabled;
        }
    }

    /// `EnableKeyboard` (`0x776ec0`): the char and key bits the key walk filters on.
    pub fn set_keyboard_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.keyboard_enabled = enabled;
        }
    }

    /// `IsKeyboardEnabled` (`0x776f90`).
    pub fn is_keyboard_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.keyboard_enabled)
    }

    /// `EnableMouseWheel`, the wheel's own bit, separate from the mouse's.
    pub fn set_mouse_wheel_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.mouse_wheel_enabled = enabled;
        }
    }

    pub fn is_mouse_wheel_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.mouse_wheel_enabled)
    }

    pub fn is_mouse_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).map(|f| f.mouse_enabled).unwrap_or(false)
    }

    /// `SetClampedToScreen` (`0x776c00`).
    pub fn set_clamped_to_screen(&mut self, h: FrameHandle, clamp: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.clamped_to_screen = clamp;
        }
    }

    /// `IsClampedToScreen` (`0x776cb0`).
    pub fn is_clamped_to_screen(&self, h: FrameHandle) -> bool {
        self.frame(h).map(|f| f.clamped_to_screen).unwrap_or(false)
    }

    /// `SetHitRectInsets`, `[left, right, top, bottom]`.
    pub fn set_hit_rect_insets(&mut self, h: FrameHandle, insets: [f32; 4]) {
        if let Some(f) = self.frame_mut(h) {
            f.hit_rect_insets = insets;
        }
    }

    /// `GetHitRectInsets`, `[left, right, top, bottom]`.
    pub fn hit_rect_insets(&self, h: FrameHandle) -> [f32; 4] {
        self.frame(h).map(|f| f.hit_rect_insets).unwrap_or([0.0; 4])
    }

    // ── Small helpers ────────────────────────────────────────────────────────────────────────────

    /// Whether `maybe_ancestor` is on `node`'s parent chain (`0x767010`, read the other way round).
    pub fn is_ancestor(&self, maybe_ancestor: FrameHandle, node: FrameHandle) -> bool {
        let mut cur = self.frame(node).and_then(|f| f.parent);
        let mut guard = 0usize;
        while let Some(c) = cur {
            if c == maybe_ancestor {
                return true;
            }
            cur = self.frame(c).and_then(|f| f.parent);
            guard += 1;
            if guard > self.frames.slots.len() {
                break; // a well-formed tree never loops
            }
        }
        false
    }

    fn parent_visible(&self, h: FrameHandle) -> bool {
        match self.frame(h).and_then(|f| f.parent) {
            Some(p) => self.frame(p).is_none_or(|pf| pf.effective_visible),
            None => true,
        }
    }

    fn parent_effective_scale(&self, h: FrameHandle) -> f32 {
        match self.frame(h).and_then(|f| f.parent) {
            Some(p) => self.frame(p).map_or(1.0, |pf| pf.effective_scale),
            None => 1.0,
        }
    }
}
