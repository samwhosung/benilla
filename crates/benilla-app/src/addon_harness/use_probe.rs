//! The use column: drive real pointer input (hover, left and right click, drag) at the frames the
//! addon itself painted, through [`UiScript::mouse_move`] and [`UiScript::mouse_button`] as the
//! app does, and report what raised. An addon can load, run its handlers and draw, yet raise the
//! moment it is hovered.
//!
//! A row that drove nothing is [`Used::Untouched`], never a pass; [`UseReport::driven`] prints
//! beside every verdict.
//!
//! Targets come from [`super::render`]'s attribution, hit-tested, and a target is kept only when
//! the frame that captures the point is one the addon created: driving a pre-existing frame would
//! charge its raise to whichever addon is in the VM. So an addon that only hooks existing frames
//! (`PlayerFrame:SetScript("OnEnter", …)`) reads `untouched` here.
//!
//! A raise means a player doing that would see the error; it does not say whose code is at fault.

use std::collections::HashSet;

use benilla_ui::script::UiScript;
use benilla_ui::widget::FrameHandle;

use super::render::{Painted, RenderBaseline};

/// How many distinct frames one addon gets driven; each event pays a full hit-test. Targets past
/// it, in painter order, are dropped, so a broken ninth widget reads clean.
pub const MAX_USE_TARGETS: usize = 8;

/// How far the drag travels: past `cursor::DRAG_START_THRESHOLD` (4 px, strict `>`) so
/// `OnDragStart` fires, and inside a 16 px item slot so the release lands on the same frame.
const DRAG_PX: f32 = 5.0;

/// Where the cursor is parked at the end, off-screen so the last `OnLeave` always fires.
const PARKED: (f32, f32) = (-10_000.0, -10_000.0);

/// What happened when the addon's own UI was used; unordered, since "raised" and "nothing to
/// touch" do not rank.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Used {
    /// The addon painted nothing a pointer can reach: neither a pass nor a failure.
    #[default]
    Untouched,
    /// Driven, and it raised.
    Raised,
    /// Driven, and nothing raised.
    Survived,
}

impl Used {
    /// The one-word form for a report row.
    pub fn word(self) -> &'static str {
        match self {
            Used::Untouched => "untouched",
            Used::Raised => "raised",
            Used::Survived => "ok",
        }
    }
}

/// What one addon's UI did when it was used.
#[derive(Debug, Clone, Default)]
pub struct UseReport {
    /// Frames actually driven; zero is [`Used::Untouched`], not a pass.
    pub driven: usize,
    /// Distinct addon frames that answered a hit-test, before [`MAX_USE_TARGETS`]; more than
    /// [`Self::driven`] means the cap dropped some.
    pub touchable: usize,
    /// The names of the frames driven, anonymous ones omitted.
    pub frames: Vec<String>,
    /// Errors raised by the input, verbatim, in the order they happened.
    pub errors: Vec<String>,
}

impl UseReport {
    pub fn verdict(&self) -> Used {
        match (self.driven, self.errors.is_empty()) {
            (0, _) => Used::Untouched,
            (_, false) => Used::Raised,
            (_, true) => Used::Survived,
        }
    }
}

/// Drive hover, both clicks and a drag at everything the addon drew, and report what raised.
///
/// Runs after every other column is read, so its input perturbs none of them, and before the
/// method oracle, whose widgets must never become input targets. `painted` is
/// [`super::render`]'s attribution, not a fresh walk.
pub(super) fn measure_use(
    script: &mut UiScript,
    baseline: &RenderBaseline,
    painted: &[Painted],
) -> UseReport {
    let mut report = UseReport::default();
    // Hide the tooltip `measure_render` left shown first: in the TOOLTIP strata it would capture
    // every hit-test beneath it, as in play it never does. Done before the error baseline, so an
    // `OnHide` hook's raise is not charged to input.
    let _ = script.run(
        r#"
        if GameTooltip and type(GameTooltip.Hide) == "function" then pcall(function() GameTooltip:Hide() end) end
    "#,
    );
    // Painted quads resolved to the frames a cursor over them would hit. `owners` dedupes the
    // work (many quads on one button), `hits` the targets (a frame driven twice double-counts).
    script.resolve();
    let mut owners: HashSet<FrameHandle> = HashSet::new();
    let mut hits: HashSet<FrameHandle> = HashSet::new();
    let mut targets: Vec<(FrameHandle, (f32, f32))> = Vec::new();
    for p in painted {
        if !owners.insert(p.owner) {
            continue;
        }
        let Some(hit) = script.hit_test_frame(p.point.0, p.point.1) else {
            continue; // painted, but nothing there takes the mouse
        };
        // A pre-existing frame captured the point: its raise is not this addon's.
        if baseline.is_pre_existing(hit) {
            continue;
        }
        if !hits.insert(hit) {
            continue;
        }
        targets.push((hit, p.point));
    }
    report.touchable = targets.len();
    targets.truncate(MAX_USE_TARGETS);
    report.driven = targets.len();
    report.frames = targets
        .iter()
        .filter_map(|(fh, _)| script.frame_name(*fh))
        .collect();

    // The engine collects handler raises itself (`UiScript::push_error`); the tail is the capture.
    let before = script.errors().len();
    for (_, (x, y)) in &targets {
        drive_one(script, *x, *y);
    }
    // Close the hover pair on the last target.
    script.mouse_move(PARKED.0, PARKED.1);
    report.errors = script.errors().split_off(before);
    report
}

/// One target, four gestures through the real input path: hover, left click, right click, drag.
///
/// A [`UiScript::resolve`] follows each, since a handler may move, show or build frames. On a
/// frame without `RegisterForDrag` the drag is a second click inside the 300 ms window, so it
/// fires `OnDoubleClick`. The gestures overlap (the drag's move also hovers, its press also
/// clicks); the order is a player's, hover before click, and the tests pin the removal of every
/// move or every left-button transition.
fn drive_one(script: &mut UiScript, x: f32, y: f32) {
    // Hover.
    script.mouse_move(x, y);
    script.resolve();
    // Left click: press and release on the same frame, as `wants_click` requires.
    script.mouse_button(x, y, "LeftButton", true);
    script.mouse_button(x, y, "LeftButton", false);
    script.resolve();
    // Right click: reaches `OnClick` only where the widget registered it, as container slots do.
    script.mouse_button(x, y, "RightButton", true);
    script.mouse_button(x, y, "RightButton", false);
    script.resolve();
    // Drag.
    script.mouse_button(x, y, "LeftButton", true);
    script.mouse_move(x + DRAG_PX, y);
    script.mouse_button(x + DRAG_PX, y, "LeftButton", false);
    script.resolve();
}
