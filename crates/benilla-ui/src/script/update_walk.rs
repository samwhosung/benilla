//! The update walk (`0x765650`), each linked frame's per-frame update in the reference's order: the
//! nine strata from the bottom up (`0x7656c8`–`0x765793`), each stratum's levels from 0 up
//! (the bound re-read after each level, `0x765774`), and each level's list of linked frames from its
//! tail (`0x7656e5`–`0x765707`). Every link is a head insert (`0x7648c9`), so a level walks oldest
//! link first: the order of [`Frame::insertion_seq`](crate::widget::Frame::insertion_seq).
//!
//! The walk is live. It saves the next frame before each call (`0x765707`), and a hide that
//! unlinks that frame moves the cursor past it (`0x76498f`), so a frame hidden before its turn is
//! not updated. A frame linked while its own level is being walked is staged until that level
//! ends (`0x76489b`, spliced at `0x76573e`) and so waits a tick, as does one linked into a level
//! already walked; one linked into a level still ahead is updated there, after that level's
//! older links.
//!
//! A frame's turn is its kind's update (`vtable+0x38`, `0x765710`): the `OnUpdate`, then, for a
//! kind with a [`KindUpdate`], that kind's engine work, so whatever it fires runs in the frame's
//! place, ahead of every frame walked after it.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::widget::{FrameHandle, KindUpdate};

use super::{editbox, event, messageframe, scrollframe, tooltip, UiScript};

/// A place in the walk: stratum, level, then within the level the frames linked before the walk
/// began (0, by link stamp) ahead of those linked during it (1, by the order they linked).
type Place = (u8, u16, u8, u64);

impl UiScript {
    /// Run the walk: each frame's `OnUpdate(self, elapsed)` (`0x76b2c0`, reading the handler when
    /// its turn comes), then its kind's own update ([`Self::kind_update`]).
    pub(super) fn update_walk(&mut self, elapsed: f32) {
        let mut ahead: BTreeMap<Place, FrameHandle> = BTreeMap::new();
        let (renumbers, mut joins, mut seen) = {
            let mut model = self.model_mut();
            // `SetScript` keeps the OnUpdate list; a destroyed frame's handle compacts out here.
            if model
                .on_update_frames
                .iter()
                .any(|&h| model.arena.frame(h).is_none())
            {
                let arena = &model.arena;
                let live: Vec<FrameHandle> = model
                    .on_update_frames
                    .iter()
                    .copied()
                    .filter(|&h| arena.frame(h).is_some())
                    .collect();
                model.on_update_frames = live;
            }
            // Only frames with an `OnUpdate` or a kind's own update do anything when walked.
            let mut seen = HashSet::new();
            for &h in model
                .on_update_frames
                .iter()
                .chain(model.arena.walked_kinds())
            {
                if let Some(f) = model.arena.frame(h).filter(|f| f.effective_visible) {
                    let place = (f.strata as u8, f.level, 0, u64::from(f.insertion_seq));
                    ahead.insert(place, h);
                    seen.insert(h);
                }
            }
            model.arena.open_link_log();
            (model.arena.renumbers(), model.on_update_joins, seen)
        };
        // Each frame linked during the walk, by the number of its latest link.
        let mut relinked: HashMap<FrameHandle, u64> = HashMap::new();
        let mut links = 0u64;
        while let Some((place, h)) = ahead.pop_first() {
            let (on_update, kind_update) = {
                let mut model = self.model_mut();
                let Some(f) = model.arena.frame(h) else {
                    continue;
                };
                // Still at this place: linked, and not relinked since the place was taken.
                let here = f.effective_visible
                    && match place.2 {
                        0 => !relinked.contains_key(&h),
                        _ => relinked.get(&h) == Some(&place.3),
                    };
                // Fixed before the `OnUpdate` runs: one virtual call does both, so a frame that
                // hides itself in its `OnUpdate` still takes its kind's update.
                let kind_update = f.kind.kind_update().filter(|_| here);
                let on_update = here
                    && model
                        .scripts
                        .get(&h)
                        .is_some_and(|s| s.contains("OnUpdate"));
                (on_update.then(|| model.frame_id(h)), kind_update)
            };
            if let Some(id) = on_update {
                if let Err(e) = event::fire_update_handler(&self.lua, id, elapsed) {
                    self.push_error(e);
                }
            }
            if let Some(k) = kind_update {
                self.kind_update(k, h, elapsed);
            }
            let mut model = self.model_mut();
            for linked in model.arena.take_linked() {
                links += 1;
                relinked.insert(linked, links);
                let Some(f) = model.arena.frame(linked) else {
                    continue;
                };
                if (f.strata as u8, f.level) > (place.0, place.1) {
                    ahead.insert((f.strata as u8, f.level, 1, links), linked);
                }
            }
            // A frame that gained an `OnUpdate` keeps its place if the walk has not passed it. Its
            // stamp compares with the walk's only while no renumber came between; else it waits.
            if model.on_update_joins != joins {
                joins = model.on_update_joins;
                if model.arena.renumbers() == renumbers {
                    let joined: Vec<FrameHandle> = model
                        .on_update_frames
                        .iter()
                        .copied()
                        .filter(|h| !seen.contains(h) && !relinked.contains_key(h))
                        .collect();
                    for j in joined {
                        seen.insert(j);
                        let Some(f) = model.arena.frame(j).filter(|f| f.effective_visible) else {
                            continue;
                        };
                        let at = (f.strata as u8, f.level, 0, u64::from(f.insertion_seq));
                        if at > place {
                            ahead.insert(at, j);
                        }
                    }
                }
            }
        }
        self.model_mut().arena.close_link_log();
    }

    /// A kind's engine work after its `OnUpdate`, in its place in the walk.
    fn kind_update(&mut self, kind: KindUpdate, h: FrameHandle, elapsed: f32) {
        match kind {
            // The flush (`0x77a7a1`), so the events an edit raised fire in the box's place, then
            // the caret blink.
            KindUpdate::EditBox => editbox::update(&self.lua, h, elapsed),
            // The fade (`0x530b9c`), whose end hides the tooltip in its place.
            KindUpdate::Tooltip => tooltip::update(&self.lua, h, elapsed),
            // The scene clock (`0x76d843`), whose completions fire `OnAnimFinished` here.
            KindUpdate::Model => self.advance_pane(h, elapsed),
            // The line fades (`0x7862b0`, `0x788476`), counted only while the frame is shown.
            KindUpdate::MessageFrame | KindUpdate::ScrollingMessageFrame => {
                messageframe::update(&mut self.model_mut(), h, elapsed)
            }
            // A marked range's recompute (`0x787152`), whose change fires `OnScrollRangeChanged`.
            KindUpdate::ScrollFrame => scrollframe::update(&self.lua, h),
        }
    }
}
