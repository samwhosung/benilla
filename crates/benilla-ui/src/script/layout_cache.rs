//! The layout cache: the reference writes a per-character `layout-cache.txt` at logout and seats
//! the user-placed frames from it at load (userPlaced is `frame+0xb4 & 0x1000`). This is the
//! engine half, a snapshot out, a restore in and a dirty bit; the app owns the file
//! (`benilla_app::ui_layout`).
//!
//! A saved frame carries its name, size and whole anchor list. An anchor's target is the screen
//! root (saved as no target, the `nil` `GetPoint` answers there) or a named frame; a frame anchored
//! to anything else is skipped whole, never half-restored.

use crate::layout::Anchor;
use crate::widget::FrameHandle;

use super::object::{point_from_str, point_name};
use super::{Model, SCREEN};

/// One anchor of a saved frame, in the spelling `GetPoint`/`SetPoint` use.
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

/// One user-placed frame's persisted geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameLayout {
    /// The frame's global name, which addresses the row across sessions.
    pub name: String,
    /// The frame's authored width (`LayoutInput::width`), which a resize drag writes.
    pub width: f32,
    /// The frame's authored height.
    pub height: f32,
    /// Every anchor the frame carries, in `GetPoint` order.
    pub points: Vec<LayoutPoint>,
}

impl super::UiScript {
    /// Snapshot, sorted by name so the file is stable, every frame the reference's writer
    /// (`0x490e60`) writes: a non-empty name (`0x490e79`, `0x490e82`), `userPlaced` (`0x490e8e`,
    /// `0x1000`) and `movable` or `resizable` (`0x490e97`, `0x100|0x200`), with every anchor
    /// nameable. Every drag stamps `userPlaced` (`0x7652b0` at `0x7652e5`), so the last test is
    /// what drops the row of a stock frame an addon once moved, once that addon is gone.
    pub fn user_placed_layouts(&self) -> Vec<FrameLayout> {
        let model = self.model_ref();
        let mut out: Vec<FrameLayout> = model
            .arena
            .iter_frames()
            .filter(|(_, f)| f.user_placed && (f.movable || f.resizable))
            .filter_map(|(h, f)| {
                let name = f.name.clone()?;
                let input = model.layout_inputs.get(&h)?;
                let points = input
                    .anchors
                    .iter()
                    .map(|a| saved_point(&model, a))
                    .collect::<Option<Vec<_>>>()?;
                Some(FrameLayout {
                    name,
                    width: input.width,
                    height: input.height,
                    points,
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Seat saved geometry back onto the frames that exist, each row whole or not at all. The
    /// apply is per arm (`0x4905e0`): the position needs `movable` (`0x490600`) and anchors (the
    /// no-position bail, `0x490613`), the size needs `resizable` (`0x490689`) and a non-zero size
    /// (`0x49069a`), and each arm that applies stamps `userPlaced` (`0x49067e`, `0x490706`), which
    /// keeps `UIParent_ManageFramePositions` off the frame. Marks nothing dirty: the values came
    /// from the file.
    pub fn restore_user_placed_layouts(&mut self, layouts: impl IntoIterator<Item = FrameLayout>) {
        let mut model = self.model_mut();
        for l in layouts {
            let Some(h) = model.arena.lookup(&l.name) else {
                continue; // a window this build does not have
            };
            let mut anchors: Vec<Anchor> = Vec::with_capacity(l.points.len());
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
                        anchors.push(Anchor::new(point, rel, rp, p.x, p.y));
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
            // The two arm gates, read before the layout input is borrowed mutably.
            let Some((movable, resizable)) = model.arena.frame(h).map(|f| (f.movable, f.resizable))
            else {
                continue;
            };
            let seat_position = movable && !anchors.is_empty();
            let seat_size = resizable && (l.width != 0.0 || l.height != 0.0);
            if !seat_position && !seat_size {
                continue;
            }
            let Some(input) = model.layout_inputs.get_mut(&h) else {
                continue;
            };
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
            // Only an arm that applied stamps, so a row neither arm took falls out of the file.
            if seat_position || seat_size {
                if let Some(f) = model.arena.frame_mut(h) {
                    f.user_placed = true;
                }
            }
            if changed {
                // The whole graph, not the frame: a restored row may repoint an anchor.
                model.touch_layout();
            }
        }
    }

    /// Whether a user-placed frame moved or resized since the last call: the host's cue to persist.
    pub fn take_user_placed_change(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().user_placed_changed)
    }
}

/// An anchor's saved form, or `None` when its target is an unnamed frame or a region.
fn saved_point(model: &Model, a: &Anchor) -> Option<LayoutPoint> {
    let relative_to = if a.relative_to == SCREEN {
        None
    } else {
        let h: FrameHandle = model.id_to_frame.get(&a.relative_to).copied()?;
        Some(model.arena.frame(h)?.name.clone()?)
    };
    Some(LayoutPoint {
        point: point_name(a.point).to_owned(),
        relative_to,
        relative_point: point_name(a.relative_point).to_owned(),
        x: a.x_off,
        y: a.y_off,
    })
}
