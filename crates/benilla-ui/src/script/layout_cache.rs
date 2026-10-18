//! The layout cache: the reference writes a per-character `layout-cache.txt` at logout and seats
//! the user-placed frames from it at load (userPlaced is `frame+0xb4 & 0x1000`). This is the
//! engine half, a snapshot out, a restore in and a dirty bit; the app owns the file
//! (`benilla_app::ui_layout`).
//!
//! A saved frame is its rect, as the reference's writer (`0x490e60`) takes it: the frame level,
//! the top-left against the screen's top-left and the extent, in the frame's own units and rounded
//! to integers. The apply (`0x4905e0`) pins TOPLEFT on the screen there and sets the size, so the
//! frame's authored anchors do not survive it.

use crate::layout::{Anchor, Anchors, Point};
use crate::widget::FrameHandle;

use super::object::point_from_str;
use super::object::{eff_scale, settle_frame};
use super::SCREEN;

/// One user-placed frame's persisted geometry, one field per line of the reference's file. A
/// field is `None` where its line is absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameLayout {
    /// The frame's global name, which addresses the row across sessions (`Frame:`).
    pub name: String,
    /// The frame level (`FrameLevel:`, `frame+0xc4`), written for every row (`0x490f55`).
    pub level: Option<u16>,
    /// The rect's left, against the screen's left (`X:`), written for a movable frame
    /// (`0x490f99`).
    pub x: Option<i32>,
    /// The rect's top, against the screen's top (`Y:`), so zero or below.
    pub y: Option<i32>,
    /// The rect's width (`W:`), written for a resizable frame (`0x49109f`).
    pub width: Option<i32>,
    /// The rect's height (`H:`).
    pub height: Option<i32>,
}

/// One anchor of a frame saved in benilla's earlier anchor form, in the spelling
/// `GetPoint`/`SetPoint` use.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutPoint {
    /// The point on the saved frame (`"BOTTOMLEFT"`, …).
    pub point: String,
    /// The target frame's name; `None` is the screen root, which `GetPoint` reports as `nil`.
    pub relative_to: Option<String>,
    /// The point on the target.
    pub relative_point: String,
    /// `CAnchor+0x4`, in the frame's own pre-scale units.
    pub x: f32,
    /// `CAnchor+0x8`.
    pub y: f32,
}

/// A row of the anchor form benilla wrote before it saved the reference's rect: the authored
/// size and the whole anchor list. Read once, to carry a player's windows into the rect form.
#[derive(Clone, Debug, PartialEq)]
pub struct AnchorLayout {
    /// The frame's global name.
    pub name: String,
    /// The frame's authored width (`LayoutInput::width`).
    pub width: f32,
    /// The frame's authored height.
    pub height: f32,
    /// Every anchor the frame carried, in `GetPoint` order.
    pub points: Vec<LayoutPoint>,
}

impl super::UiScript {
    /// Snapshot, in creation order as the writer walks the list of every frame (`0x490e24`,
    /// `[root+0xcc4]`, appended by the constructor at `0x76930d`), every frame it writes: a
    /// non-empty name (`0x490e79`, `0x490e82`), `userPlaced` (`0x490e8e`, `0x1000`) and `movable`
    /// or `resizable` (`0x490e97`, `0x100|0x200`), with a rect to read. Every drag stamps
    /// `userPlaced` (`0x7652b0` at `0x7652e5`), so the last test is what drops the row of a stock
    /// frame an addon once moved, once that addon is gone.
    ///
    /// The rect is the one the frame holds, stale or not (`0x768320`, its valid bit): only a frame
    /// holding none is resolved now (`0x490eed`, `0x7680e0(1)`) and read again, and still none
    /// writes no row (`0x490f00`). The root's rect is the screen, which is always held, so the
    /// second bail (`0x490f17`) never takes here. The level and the two flags are read after the
    /// rect (`0x490f55`, `0x490f93`, `0x491099`), since a resolve can run script.
    ///
    /// `X:`/`Y:` round half away from zero (`0x490fc4`–`0x490fdf`, then `0x40a2b0`'s truncating
    /// `fistp`); `W:`/`H:` add a half and truncate (`0x4910cd`–`0x4910d3`).
    pub fn user_placed_layouts(&self) -> Vec<FrameLayout> {
        let mut candidates: Vec<(u64, FrameHandle, String)> = self
            .model_ref()
            .arena
            .iter_frames()
            .filter(|(_, f)| f.user_placed && (f.movable || f.resizable))
            .filter_map(|(h, f)| {
                let name = f.name.clone().filter(|n| !n.is_empty())?;
                Some((f.created(), h, name))
            })
            .collect();
        candidates.sort_unstable_by_key(|c| c.0);
        let mut out = Vec::with_capacity(candidates.len());
        for (_, h, name) in candidates {
            if !self.model_ref().frame_rects.contains_key(&h) {
                settle_frame(self.lua(), h);
            }
            let model = self.model_ref();
            let Some(r) = model.frame_rect(h) else {
                continue;
            };
            let Some((movable, resizable, level)) = model
                .arena
                .frame(h)
                .map(|f| (f.movable, f.resizable, f.level))
            else {
                continue; // the resolve's script destroyed it
            };
            let root = model.screen;
            let scale = f64::from(eff_scale(&model, h));
            let own = |v: f32| f64::from(v) / scale;
            let (x, y) = if movable {
                (
                    Some(round_offset(own(r.left - root.left))),
                    Some(round_offset(own(r.top - root.top))),
                )
            } else {
                (None, None)
            };
            let (width, height) = if resizable {
                (
                    Some(round_extent(own(r.right - r.left))),
                    Some(round_extent(own(r.top - r.bottom))),
                )
            } else {
                (None, None)
            };
            out.push(FrameLayout {
                name,
                level: Some(level),
                x,
                y,
                width,
                height,
            });
        }
        out
    }

    /// Seat saved rows back onto the frames that exist, as the apply (`0x4905e0`) does. A level of
    /// zero or more is set, carried to same-strata children (`0x4905ee`–`0x4905f5`). The position
    /// needs `movable` (`0x490600`) and a non-zero `X` or `Y` (`0x490607`–`0x490613`): it clears
    /// every anchor (`0x49061a`) and pins TOPLEFT on the screen's TOPLEFT there (`0x490670`). The
    /// size needs `resizable` (`0x490689`) and a non-zero `W` or `H` (`0x49068e`–`0x49069a`), and
    /// sets each non-zero one (`0x4906c7`, `0x4906fa`). Each arm that applies stamps `userPlaced`
    /// (`0x49067e`, `0x490706`), which keeps `UIParent_ManageFramePositions` off the frame. Each
    /// row ends in a forced resolve of its frame, shown or hidden (`0x490710`, `0x7680e0(1)`), so
    /// its `OnSizeChanged` fires before the next row applies. Marks nothing dirty: the values came
    /// from the file.
    pub fn restore_user_placed_layouts(&mut self, layouts: impl IntoIterator<Item = FrameLayout>) {
        for l in layouts {
            if let Some(h) = self.apply_layout_row(l) {
                settle_frame(self.lua(), h);
            }
        }
    }

    /// One row of [`Self::restore_user_placed_layouts`], up to its invalidate; the frame it
    /// applied to.
    fn apply_layout_row(&mut self, l: FrameLayout) -> Option<FrameHandle> {
        let mut model = self.model_mut();
        // A window this session does not have applies nothing (`0x4905e6`).
        let h = model.arena.lookup(&l.name)?;
        if let Some(level) = l.level {
            model.arena.set_frame_level(h, level, true);
        }
        let (movable, resizable) = model.arena.frame(h).map(|f| (f.movable, f.resizable))?;
        // An absent line reads as the reference's cleared accumulator, zero.
        let (x, y) = (l.x.unwrap_or(0), l.y.unwrap_or(0));
        let (w, ht) = (l.width.unwrap_or(0), l.height.unwrap_or(0));
        let seat_position = movable && (x != 0 || y != 0);
        let seat_size = resizable && (w != 0 || ht != 0);
        let input = model.layout_inputs.entry(h).or_default();
        let mut changed = false;
        if seat_position {
            let pin = Anchors::from([Anchor::new(
                Point::TopLeft,
                SCREEN,
                Point::TopLeft,
                x as f32,
                y as f32,
            )]);
            changed |= !input.anchors.bits_eq(&pin);
            input.anchors = pin;
        }
        if seat_size && w != 0 {
            changed |= input.width.to_bits() != (w as f32).to_bits();
            input.width = w as f32;
        }
        if seat_size && ht != 0 {
            changed |= input.height.to_bits() != (ht as f32).to_bits();
            input.height = ht as f32;
        }
        if seat_size {
            model.note_authored_size(h);
        }
        if seat_position || seat_size {
            if let Some(f) = model.arena.frame_mut(h) {
                f.user_placed = true;
            }
        }
        // Every row that names a frame ends in an invalidate (`0x490710`).
        model.invalidate_frame(h, changed);
        Some(h)
    }

    /// Seat rows of benilla's earlier anchor form, each whole or not at all, under the apply's arm
    /// gates (`0x490600`, `0x490689`) and stamps (`0x49067e`, `0x490706`). The next save writes
    /// the frames back in the rect form.
    pub fn restore_anchor_layouts(&mut self, layouts: impl IntoIterator<Item = AnchorLayout>) {
        let mut model = self.model_mut();
        for l in layouts {
            let Some(h) = model.arena.lookup(&l.name) else {
                continue;
            };
            let mut anchors = Anchors::new();
            let mut usable = true;
            for p in &l.points {
                // `frame_id` mints the id lazily, so a target nothing has anchored to yet resolves.
                let rel = match &p.relative_to {
                    None => Some(SCREEN),
                    Some(n) => model.arena.lookup(n).map(|t| model.frame_id(t)),
                };
                match (
                    point_from_str(&p.point),
                    rel,
                    point_from_str(&p.relative_point),
                ) {
                    (Some(point), Some(rel), Some(rp)) => {
                        anchors.insert(Anchor::new(point, rel, rp, p.x, p.y));
                    }
                    _ => {
                        usable = false;
                        break;
                    }
                }
            }
            if !usable {
                continue;
            }
            let Some((movable, resizable)) = model.arena.frame(h).map(|f| (f.movable, f.resizable))
            else {
                continue;
            };
            let seat_position = movable && !anchors.is_empty();
            let seat_size = resizable && (l.width != 0.0 || l.height != 0.0);
            if !seat_position && !seat_size {
                continue;
            }
            let input = model.layout_inputs.entry(h).or_default();
            let mut changed = false;
            if seat_position {
                changed |= input.anchors != anchors;
                input.anchors = anchors;
            }
            if seat_size {
                changed |= input.width.to_bits() != l.width.to_bits()
                    || input.height.to_bits() != l.height.to_bits();
                input.width = l.width;
                input.height = l.height;
            }
            if let Some(f) = model.arena.frame_mut(h) {
                f.user_placed = true;
            }
            model.invalidate_frame(h, changed);
        }
    }

    /// Whether a user-placed frame moved or resized since the last call: the host's cue to persist.
    pub fn take_user_placed_change(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().user_placed_changed)
    }
}

/// `X:`/`Y:`'s rounding: a half added, or taken off below zero (`fcom 0.0; test ah,0x5; jp`),
/// then truncated toward zero (`0x40a2b0`), so half rounds away from zero.
fn round_offset(v: f64) -> i32 {
    (if v < 0.0 { v - 0.5 } else { v + 0.5 }).trunc() as i32
}

/// `W:`/`H:`'s rounding: a half added with no sign test, then truncated toward zero.
fn round_extent(v: f64) -> i32 {
    (v + 0.5).trunc() as i32
}

#[cfg(test)]
mod tests {
    use super::{round_extent, round_offset};

    #[test]
    fn offsets_round_half_away_from_zero_and_extents_add_a_half() {
        assert_eq!(
            [2.5, 2.49, -2.5, -2.49, 0.0, -0.4].map(round_offset),
            [3, 2, -3, -2, 0, 0]
        );
        // No sign test on an extent: a negative half truncates toward zero.
        assert_eq!([2.5, 2.49, 0.0, -0.7].map(round_extent), [3, 2, 0, 0]);
    }
}
