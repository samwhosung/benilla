//! [`WidgetArena`]'s propagation mutators — the binary-verified subtree math that keeps
//! `effective_visible`/`strata`/`level`/`effective_scale`/`effective_alpha` (and reparenting, which
//! touches several of those at once) consistent whenever a frame's own state changes. Split out of
//! [`super`] because this is the module's dense, ground-truth-heavy core (see the module doc's
//! "Ground truth" section); the arena's storage/create/destroy/read-access lives with the struct
//! definition in [`super`].

use crate::order::Strata;

use super::{FrameHandle, RegionHandle, WidgetArena, SCALE_EPS};

impl WidgetArena {
    // ── Visibility (effective_visible_show/_hide) ────────────────────────────────────────────────

    /// Set a frame's own `shown` bit and propagate effective visibility through its subtree, after
    /// show `0x76ae10` / hide `0x76ad50`. Returns, in pre-order (a node before its descendants),
    /// **every frame whose `effective_visible` actually changed** — the caller fires
    /// `OnShow`/`OnHide` for those, in order.
    ///
    /// **Two recorded deviations from `0x76ae10`, both in this signature**. The
    /// reference is **post-order** — it marks itself visible, recurses into its children, and fires
    /// its own notify last (`0x76aef5`, past both child loops) — so descendants are notified before
    /// ancestors. And it keeps **no snapshot**: each loop re-reads the live links after every
    /// per-node call, so a frame hidden by a handler mid-cascade refuses at its own gate
    /// (`0x76ae1d`) and is never notified, where the list returned here already holds it. The
    /// auction window exercises exactly that case and lands on the same state by the other road;
    /// 2317 has the measurements and the risk for whoever changes this. A no-op `shown` write, or
    /// a change that does not move any effective visibility (e.g. hiding an already-invisible frame),
    /// returns empty.
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
            // Transition-gated: no change here ⇒ no change below (children's parent-visibility and
            // own shown are unchanged). Prune, exactly like the client's recursion.
            return;
        }
        self.frame_mut(h).unwrap().effective_visible = new_ev;
        if new_ev {
            // The client re-ADDS the frame to its (strata, level) bucket on this transition —
            // an intrusive-list append, so a newly shown frame draws over everything already in
            // its bucket ([`WidgetArena::resequence_to_tail`]). Pre-order recursion gives the
            // descendants later seqs than the node, exactly like the client's recursion re-adding
            // each as its own effectiveVisible flips.
            self.resequence_to_tail(h);
        }
        changed.push(h);
        for c in children {
            self.propagate_visible(c, new_ev, changed);
        }
    }

    // ── Strata (set_frame_strata) ────────────────────────────────────────────────────────────────

    /// Force `h` and its **whole subtree** to `strata`, per `set_frame_strata 0x76a470`: set the
    /// frame's strata, then recursively set every child frame to the same value. The client's
    /// visible-gated bucket remove/add (its live draw-list maintenance) is mirrored by
    /// re-sequencing a visible frame to its new bucket's tail
    /// ([`WidgetArena::resequence_to_tail`]); [`crate::order::traversal`] recomputes the buckets
    /// from the fields.
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

    // ── Level (set_frame_level) ──────────────────────────────────────────────────────────────────

    /// Set `h`'s level. When `propagate`, **same-strata** child frames shift by the same delta
    /// (relative offsets preserved); cross-strata children are untouched — per `set_frame_level
    /// 0x76a4f0`. Level is unsigned here, so the client's `if (level < 0) level =
    /// 0` clamp is subsumed by the type (a Lua binding saturates on conversion); a propagated shift
    /// that would go negative saturates to 0, and an overflow saturates to `u16::MAX`.
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
        // The client's visible-gated remove/add: a visible frame lands at its NEW bucket's tail
        // (a hidden one is in no bucket — it appends on its next show anyway).
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
                _ => continue, // cross-strata (or stale) child: untouched
            };
            self.set_frame_level(c, child_new, true);
        }
    }

    /// Squeeze the free levels out of `strata` and return the level counter — level compaction
    /// (`0x764eb0`), the step `CSimpleTop::Raise 0x7650f0` runs immediately before it sets the
    /// raised frame's level to `bucket->count(+0x8)`.
    ///
    /// **EVERY frame of the stratum is renumbered — hidden ones included, and that is the byte
    /// law, not a benilla choice**. `0x764eb0`'s occupancy test is a two-part OR: a
    /// level node's own list, else a scan of the client's **master frame list** (`[root+0xcc4]`,
    /// link offset `0x304` — every live frame, joined in the `CSimpleFrame` ctor and counted by
    /// `GetNumFrames`), and that scan filters on exactly two fields — `+0xc0` strata (`0x764f18`)
    /// and `+0xc4` level (`0x764f20`). The renumber pass `0x764f80..0x764fc9` walks the same list.
    /// A census of `+0xd4` (effectiveVisible) over the whole body `[0x764eb0, 0x76500e)` finds
    /// **zero** hits; in `set_frame_level 0x76a4f0` the `+0xc4` write is unconditional
    /// (`0x76a53b`) and `+0xd4` gates only the bucket relink below it (`0x76a541`).
    ///
    /// Gating the renumber on visibility — which benilla did, reasoning from the level lists rather
    /// than from the bytes — is not a relabelling at all: it changes the relative order of the part
    /// it touches against the part it does not, and that difference then rides out on the raise's
    /// `propagate` delta, which is computed in the *new* numbering and applied to levels still
    /// carrying the *old* one. One level of squeeze taken out of a visible child and not out of its
    /// hidden sibling becomes a permanent inversion between two frames that are level by
    /// construction — `parent.level + 1` for both. Gatherer's Report window is the shape that found
    /// it: the window's visible Close button and its hidden, mouse-enabled, full-cover body are both
    /// direct children, the body was hidden for the Show raise, and it came out one level above the
    /// button and ate every click and hover the button should have had. The reference lands that
    /// pair equal, and the hit sweep's tie then goes to the earlier-linked frame, the button (1816).
    ///
    /// So the visible gate stays where it belongs — on the *link list* (`resequence_to_tail`, draw
    /// participation) — and off the *numbering*, which is a property of the frame.
    ///
    /// **The renumber is strictly order-preserving, so by itself it changes no draw order**: distinct
    /// levels map to distinct indices monotonically and equal levels stay equal. Its whole job is to
    /// keep the raise target *bounded* — without it, `level := max + 1` would ratchet upward one step
    /// per raise for as long as the session lasts.
    ///
    /// **Ours packs tighter than the binary's, deliberately.** `bucket+0x8` is a high-water *bound*,
    /// not a count of occupied levels, and `0x764eb0`'s cursor resumes at its saved pre-gap index
    /// while that bound shrinks under it (`0x764fd3`/`0x764fd8`), so a wide gap carries an interior
    /// hole past it: occupied `{0, 4, 6}` at count 7 comes out `{0, 1, 3}` at count 4, not
    /// `{0, 1, 2}` at 3. That artifact is invisible to draw and hit order — both maps are monotone
    /// and injective on levels, and both keep the invariant the raise needs (`count` above every
    /// occupied level) — so we take the contiguous one rather than model a cursor bug.
    ///
    /// Two things it deliberately is **not**: it does not propagate (`0x764faf` passes
    /// `propagate = 0`, so a parent/child level *offset* is not preserved across a hole either —
    /// parent 5 / child 7 comes out adjacent), and **it does not relink**, which is a known
    /// divergence: the binary re-levels each moved frame through `0x76a4f0` at `0x764fb6`, so every
    /// moved *visible* frame is unlinked and re-inserted and its within-level link order is
    /// rewritten (a hidden frame takes no list churn). We write `level` directly instead, so our
    /// `insertion_seq` survives a compaction untouched. Nothing observed yet turns on it; it is
    /// named in 2104 rather than changed under a bug fix.
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

    // ── The toplevel flag (flag word bit 0x1) ────────────────────────────────────────────────────

    /// Set a frame's `toplevel` bit — `SetToplevel 0x775440` / XML `toplevel`, both through the pure
    /// bit-setter `0x76a3c0` (see [`Frame::toplevel`]). A pure flag write: **it raises nothing**.
    /// A stale handle is a no-op.
    pub fn set_toplevel(&mut self, h: FrameHandle, toplevel: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.toplevel = toplevel;
        }
    }

    /// Whether the frame carries the `toplevel` bit (`IsToplevel`). A stale handle reads as `false`.
    pub fn is_toplevel(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.toplevel)
    }

    // ── Scale ────────────────────────────────────────────────────────────────────────────────────

    /// Set `h`'s own scale and recompute effective scale down the subtree, ε-gated, per
    /// `0x76ac90`: `effectiveScale = parentScale * ownScale`; a
    /// node whose new effective scale is within [`SCALE_EPS`] of its current one is skipped along
    /// with its subtree (its children's parent-scale did not move).
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

    /// Set `h`'s alpha and **overwrite every descendant frame's** with the same raw value — the
    /// 1.12 mechanism (`SetAlpha 0x76a690` writes `[this+0xc8]` then recurses the child-frame list
    /// pushing the SAME value). An eager set-time flatten, structurally the strata subtree-force —
    /// never a draw-time ancestor product, so `effective_alpha` (what a renderer's one-hop region
    /// composition reads) always equals `alpha`.
    pub fn set_alpha(&mut self, h: FrameHandle, alpha: f32) {
        let Some(f) = self.frame_mut(h) else { return };
        f.alpha = alpha;
        f.effective_alpha = alpha;
        let children = self.frame(h).expect("just wrote it").children.clone();
        for c in children {
            self.set_alpha(c, alpha);
        }
    }

    // ── Reparenting (reparent_begin / reparent_finish) ───────────────────────────────────────────

    /// The runtime reparent, phase 1 of 2 — the guards and the **hide half** of
    /// `CSimpleFrame::SetParent 0x76ab10`'s sequence (which corrects this
    /// module's previous "strata/level are NOT changed by reparenting" — refuted at `0x76ab65`).
    ///
    /// Returns `None` for the byte-verified TOTAL no-op — `newParent == currentParent` (`0x76ab20`
    /// skips *everything*) — and for a dead handle or a cycle (the Lua binding raises on a cycle
    /// *before* calling here, per the binding's own inline `+0x9c` walk at `0x7a177f`; a direct
    /// caller gets the silent reject as a backstop). Otherwise `Some(hidden)`: every frame whose
    /// effective visibility just dropped, in pre-order — empty when the frame was not effectively
    /// visible ("a reparent of a hidden frame fires neither" event). The caller fires `OnHide` for
    /// those and then calls [`Self::reparent_finish`]; the hide runs while the frame still hangs
    /// under its OLD parent, as the reference's `0x76ab41` does before the `+0x9c` store, so an
    /// `OnHide` handler observes the old parent. (One knowing divergence: the reference unlinks
    /// from the old parent's child list *before* the hide, so an `OnHide` calling
    /// `oldParent:GetChildren()` no longer sees the frame there; ours still does — the relink is
    /// phase 2's, so a handler that itself reparents can't leave the lists half-spliced.)
    pub fn reparent_begin(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
    ) -> Option<Vec<FrameHandle>> {
        self.frame(h)?;
        // Keep only a live new parent; reject a cycle (new_parent == h or below h).
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

    /// Phase 2 — the relink and the rest of `0x76ab10`'s verified sequence: **strata := parent's**
    /// (`0x76ab5a` → the value-gated subtree force of [`Self::set_frame_strata`]; MEDIUM for nil),
    /// **level := parent.level + 1 with `propagate = 0`** (`0x76ab65`; 0 for nil), scale
    /// re-inherit, then the **show half**. Returns the frames whose effective visibility came back
    /// (fire `OnShow` for those, in order).
    ///
    /// Load-bearing consequences:
    ///
    /// - **Existing children keep their absolute levels** (`propagate = 0`; level compaction
    ///   `0x764eb0`'s one caller is the toplevel Raise, unreachable from here). A child created
    ///   when the parent sat lower can land BELOW its own parent — the client ships that and
    ///   nothing repairs it but the child's own `SetFrameLevel` (which is exactly what AtlasLoot's
    ///   button templates do in `OnShow`, and why its browse panel works on the reference).
    /// - **The show half is gated on `was_visible`** (`0x76abfd`: `ebx && (parent==0 ||
    ///   parent->+0xd4)`) — the `+0xd4` cascade only runs inside `0x76ae10`, so a reparent of a
    ///   hidden (or hidden-chained) frame does NOT recompute effective visibility: moved under a
    ///   visible parent it stays effectively invisible until something shows it. That staleness is
    ///   the shipped behaviour, not an oversight to "fix" here.
    /// - The `OnShow` refire down the re-shown subtree is what lets an addon hand-repair the
    ///   propagate-0 level law (`this:SetFrameLevel(GetParent():GetFrameLevel()+1)` in `OnShow` —
    ///   the reference fires it via the `+0x84`/`+0x88` hide→show round-trip on any visible
    ///   reparent).
    ///
    /// `was_visible` is phase 1's `!hidden.is_empty()` — the reference's `ebx`, captured at
    /// `0x76ab2b` before anything runs. Alpha stays untouched throughout: the client only pushes
    /// alpha at `SetAlpha` time (module alpha section).
    pub fn reparent_finish(
        &mut self,
        h: FrameHandle,
        new_parent: Option<FrameHandle>,
        was_visible: bool,
    ) -> Vec<FrameHandle> {
        let mut shown = Vec::new();
        if self.frame(h).is_none() {
            return shown; // died inside an OnHide — nothing left to move
        }
        let new_parent = new_parent.filter(|&p| self.frame(p).is_some());
        // Relink from the CURRENT parent (an OnHide handler may itself have reparented; the
        // reference's unconditional `+0x9c` store at `0x76ab49` means the outer call wins).
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
            // SetParent(nil) is a RESET — strata 3 (MEDIUM), level 0 (`0x76aba3`/`0x76abac`) —
            // not "keep current".
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

    /// [`Self::reparent_begin`] + [`Self::reparent_finish`] in one call, for callers with no Lua
    /// to fire between the halves (tests, direct arena use). Returns both phases' visibility
    /// changes concatenated — hide half first. The Lua binding does NOT use this: it must fire
    /// `OnHide` between the phases (the events' direction is read from live state at fire time).
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

    /// Re-link a **region leaf** (Texture/FontString) to a new owner frame — the arena half of
    /// `Region:SetParent`. `None` detaches ([`super::Region::detached`]). Returns whether anything
    /// moved (a same-owner call and a stale handle both report `false`).
    ///
    /// **This is a different mechanism from the frame reparent above, and the difference is
    /// verified.** `SetParent` is one Region-table binding (`0x7a1550`) dispatching a per-class
    /// virtual: a plain Region gets `0x76c430`, which writes the geometry parent and nothing else,
    /// but a Texture or FontString gets `0x7733d0` → `0x77fd10`, a **full re-link** — remove from
    /// the old parent's draw layer (`0x77fc60`) and region list (`vtbl+0x2c`), store the new
    /// parent, insert into its region list (`0x76a750`) and re-register in its draw layer
    /// (`0x77fcb0`), **preserving layer and sub-level**.
    /// So the layer/sub-level fields are deliberately untouched here and only the membership moves;
    /// the fresh `decl_seq` is the "insert into the new parent's region list" half, which lands the
    /// region at the tail of that frame's list exactly as the client's insert does.
    ///
    /// Nothing propagates, unlike the frame reparent: a region has no effective-visible/scale/alpha
    /// of its own here — every reader (`IsVisible`, the measure key, `extract`'s alpha product)
    /// resolves the owner frame's live values through [`super::Region::owner`], so re-pointing that
    /// field *is* the propagation the client does eagerly (`0x77fd10` pushes the new parent's shown
    /// bit into the region).
    ///
    /// **A same-owner call is a no-op, and that is the conservative reading, not a verified one.**
    /// `0x77fd10` is unconditional, so the reference may re-tail a region within its own layer when
    /// an addon re-parents it to the frame it already belongs to. Not reproducing that can only
    /// ever *preserve* declared draw order, never invent one; and the corpus's only region
    /// `SetParent` caller — `FuBar_FuXPFu.lua:210-211`, which re-parents two `XPBar:CreateTexture`
    /// sparks to `XPBar` — moves both in declaration order, so the two readings agree there.
    pub fn set_region_owner(&mut self, rh: RegionHandle, new_owner: Option<FrameHandle>) -> bool {
        let Some(region) = self.region(rh) else {
            return false;
        };
        let (old_owner, was_detached) = (region.owner, region.detached);
        // Keep only a live new owner. No cycle check: the client's (`0x7a177f`) walks the new
        // parent's frame chain looking for the receiver, and a Texture/FontString is never a frame,
        // so for a region it can never fire.
        let new_owner = new_owner.filter(|&f| self.frame(f).is_some());
        match new_owner {
            Some(f) if f == old_owner && !was_detached => return false,
            None if was_detached => return false,
            _ => {}
        }
        let Some(new_owner) = new_owner else {
            // Detached: the entry stays in its last owner's list (so `destroy` still frees it) and
            // the flag alone takes it out of the draw.
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

    // ── Mouse interaction (the hit-test flag) ────────────────────────────────────────────────────

    /// Set a frame's `mouse_enabled` flag (`EnableMouse`). Only mouse-enabled *and* effective-visible
    /// frames capture the cursor in [`crate::order::hit_test`]; the default is **false** (WoW's
    /// `enableMouse` default). A stale handle is a no-op. (Keyboard focus is a separate concern, not
    /// modeled here yet.)
    pub fn set_mouse_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.mouse_enabled = enabled;
        }
    }

    /// `EnableKeyboard` (`0x776f90`) — kind-0/kind-1 bucket membership. Stored and answered, and
    /// it is what [`crate::script::keyboard`]'s delivery walk filters on (1319).
    pub fn set_keyboard_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.keyboard_enabled = enabled;
        }
    }

    /// Whether the frame is keyboard-enabled. A stale handle reads as `false`.
    pub fn is_keyboard_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.keyboard_enabled)
    }

    /// `EnableMouseWheel` — the wheel's own gate, separate from the mouse's.
    pub fn set_mouse_wheel_enabled(&mut self, h: FrameHandle, enabled: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.mouse_wheel_enabled = enabled;
        }
    }

    /// Whether the frame currently accepts the wheel. A stale handle reads as `false`.
    pub fn is_mouse_wheel_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).is_some_and(|f| f.mouse_wheel_enabled)
    }

    /// Whether the frame currently accepts the mouse (see [`WidgetArena::set_mouse_enabled`]). A
    /// stale handle reads as `false`.
    pub fn is_mouse_enabled(&self, h: FrameHandle) -> bool {
        self.frame(h).map(|f| f.mouse_enabled).unwrap_or(false)
    }

    // ── Clamp-to-screen (geometry flags bit4) ────────────────────────────────────────────────────

    /// Set a frame's clamp-to-screen flag (`SetClampedToScreen` `0x776c00` — geometry flags bit4;
    /// see [`Frame::clamped_to_screen`] for the tooltip-kind default). A stale handle is a no-op.
    pub fn set_clamped_to_screen(&mut self, h: FrameHandle, clamp: bool) {
        if let Some(f) = self.frame_mut(h) {
            f.clamped_to_screen = clamp;
        }
    }

    /// Whether the frame clamps to the screen (`IsClampedToScreen` `0x776cb0`). A stale handle
    /// reads as `false`.
    pub fn is_clamped_to_screen(&self, h: FrameHandle) -> bool {
        self.frame(h).map(|f| f.clamped_to_screen).unwrap_or(false)
    }

    // ── Hit-rect insets (the mouse rect, not the draw rect) ──────────────────────────────────────

    /// Set a frame's hit-rect insets, `[left, right, top, bottom]` (`SetHitRectInsets`; see
    /// [`Frame::hit_rect_insets`]). A stale handle is a no-op.
    pub fn set_hit_rect_insets(&mut self, h: FrameHandle, insets: [f32; 4]) {
        if let Some(f) = self.frame_mut(h) {
            f.hit_rect_insets = insets;
        }
    }

    /// A frame's hit-rect insets, `[left, right, top, bottom]` (`GetHitRectInsets`). A stale handle
    /// reads as all-zero — the same "no inset" answer an untouched frame gives.
    pub fn hit_rect_insets(&self, h: FrameHandle) -> [f32; 4] {
        self.frame(h).map(|f| f.hit_rect_insets).unwrap_or([0.0; 4])
    }

    // ── Small helpers ────────────────────────────────────────────────────────────────────────────

    /// Is `maybe_ancestor` on the parent chain of `node` (walking up, loop-guarded)? — the client's
    /// `0x767010`, read the other way round.
    ///
    /// Two callers, and they are the two the binary has: [`WidgetArena::set_parent`]'s cycle guard,
    /// and the raise's overlap scan, which excludes the raised frame's own subtree from the frames
    /// it may be considered to overlap ([`crate::script::object::toplevel`]).
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
                break; // defensive; a well-formed tree never loops
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
