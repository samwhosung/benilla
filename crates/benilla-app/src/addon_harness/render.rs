//! The render column: which painting quads come from widgets the addon created.
//!
//! [`RenderBaseline::of`] snapshots every live draw target before the addon runs, and anything
//! new afterwards is the addon's, anonymous frames included; handles are generational, so a
//! reused slot is never taken for a survivor. [`Drew`] splits a window of the addon's own from an
//! overlay on an existing one.
//!
//! It over-reports: a quad off-screen or under another window counts. It under-reports an addon
//! that only changes an existing widget in place (`SetTexture`, `SetBackdrop`), which makes no new
//! handle. The session world is minimal, so a window that opens only on unsimulated state (a
//! bank, a raid) draws nothing here.

use std::collections::{BTreeSet, HashSet};

use benilla_ui::layout::Rect;
use benilla_ui::order::ZTarget;
use benilla_ui::script::{QuadContent, UiScript};
use benilla_ui::widget::FrameHandle;

/// Every widget that existed before an addon ran, the set the render probe diffs against.
pub(super) struct RenderBaseline {
    targets: HashSet<ZTarget>,
    frames: HashSet<FrameHandle>,
}

impl RenderBaseline {
    /// Snapshot the VM. Cheap: one walk of the arena, no allocation per quad.
    pub(super) fn of(script: &UiScript) -> Self {
        let targets: HashSet<ZTarget> = script.live_targets().into_iter().collect();
        let frames = targets
            .iter()
            .filter_map(|t| match t {
                ZTarget::Frame(fh) => Some(*fh),
                ZTarget::Region(_) => None,
            })
            .collect();
        Self { targets, frames }
    }

    /// Whether `frame` existed before the addon ran; [`super::use_probe`] judges only pointer
    /// events captured by frames the addon created.
    pub(super) fn is_pre_existing(&self, frame: FrameHandle) -> bool {
        self.frames.contains(&frame)
    }
}

/// One place the addon painted, for [`super::use_probe`] to point at; emitted by the same walk
/// that attributes the render column, so the two never disagree.
pub(super) struct Painted {
    /// The frame the quad was charged to, used only to dedupe candidates: the hit-test decides
    /// which frame a click reaches.
    pub(super) owner: FrameHandle,
    /// The centre of the painted rect.
    pub(super) point: (f32, f32),
}

/// What an addon put on screen, ordered by how much of the screen it owns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Drew {
    /// No painting quad came from anything the addon created.
    #[default]
    Nothing,
    /// Painted only from widgets hung off pre-existing frames, such as `!OmniCC`'s anonymous
    /// countdown text on an action button's cooldown.
    Overlay,
    /// Painted from a frame tree of its own, rooted outside everything pre-existing.
    Own,
}

impl Drew {
    /// The one-word form for a report row.
    pub fn word(self) -> &'static str {
        match self {
            Drew::Nothing => "nothing",
            Drew::Overlay => "overlay",
            Drew::Own => "own",
        }
    }
}

/// What one addon drew, and off which of its frames.
#[derive(Debug, Clone, Default)]
pub struct RenderReport {
    /// Painting quads from frame trees the addon created and rooted itself.
    pub own_quads: usize,
    /// Painting quads from widgets the addon hung off a frame that already existed.
    pub overlay_quads: usize,
    /// Every distinct named frame the quads were charged to (nearest named ancestor), sorted and
    /// uncapped: [`MAX_NAMED_FRAMES`] bounds only the print.
    pub frames: Vec<String>,
}

impl RenderReport {
    pub fn drew(&self) -> Drew {
        match (self.own_quads, self.overlay_quads) {
            (0, 0) => Drew::Nothing,
            (0, _) => Drew::Overlay,
            _ => Drew::Own,
        }
    }
}

/// How many named frames a row prints. It bounds the display, never the collection: a set capped
/// while filling keeps an arbitrary few that then print sorted, as if complete.
pub const MAX_NAMED_FRAMES: usize = 6;

/// Open the entry points that make a UI visible, tick, then attribute every painting quad.
///
/// Unlike [`super::drive_ui_probe`], which toggles open then closed, this only opens and leaves
/// everything up. It opens the backpack, updates action button 1 and starts its cooldown, and
/// shows the tooltip, the entry points corpus addons replace or hook; then ten ticks, since much
/// addon painting happens in the first `OnUpdate`. Returns the report and the places the addon
/// painted, in painter order, for [`super::use_probe`].
pub(super) fn measure_render(
    script: &mut UiScript,
    baseline: &RenderBaseline,
) -> (RenderReport, Vec<Painted>) {
    // A real action with an icon on slot 1: cooldown-count addons write text only while the
    // button's icon is visible. Seated here, after every other column is read, so it perturbs none.
    script.set_action(
        1,
        Some(benilla_ui::script::ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Nature_Lightning".into()),
            kind: 0,
            action: 403,
            count: 0,
            consumable: false,
        }),
    );
    script.set_action_state(
        1,
        Some(benilla_ui::script::ActionState {
            usable: true,
            ..Default::default()
        }),
    );
    // A 16-slot empty backpack, as every character has: with 0 slots a bag addon draws a window
    // with no slots, the very failure this column catches.
    script.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    // Each global is called only if it is a function and each raise is swallowed: raises are the
    // `probe_errors` column's, and one here must not zero the measurements after it.
    let _ = script.run(
        r#"
        local function try(fn) if type(fn) == "function" then pcall(fn) end end
        -- OPEN, never toggle. Bagnon overrides ToggleBackpack and OpenAllBags as toggles, so
        -- calling either from an unknown state is a coin flip; its OpenBackpack override is an
        -- open, as the reference's is.
        try(OpenBackpack)
        if ActionButton1 then
            this = ActionButton1
            try(ActionButton_Update)
        end
        -- A RUNNING cooldown. Nothing else makes a cooldown-count addon paint: OmniCC's hook only
        -- builds its text frame when `start > 0 and duration > OmniCC.min and enable > 0`, and its
        -- OnUpdate only writes text while the button's icon is visible.
        if ActionButton1Cooldown and type(CooldownFrame_SetTimer) == "function" then
            try(function() CooldownFrame_SetTimer(ActionButton1Cooldown, GetTime(), 30, 1) end)
        end
        if GameTooltip and UIParent then
            try(function()
                GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
                GameTooltip:SetText("benilla render probe")
                GameTooltip:Show()
            end)
        end
        this = nil
    "#,
    );
    for _ in 0..10 {
        script.tick(0.1);
    }

    script.resolve();
    let mut report = RenderReport::default();
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut painted: Vec<Painted> = Vec::new();
    for quad in script.extract() {
        // A frame's own slot paints nothing; counting it would score every addon that merely
        // creates a frame as having drawn.
        if matches!(quad.content, QuadContent::Frame) {
            continue;
        }
        // Under-constrained widgets never reach the screen; the rect is where `use_probe` aims.
        let Some(rect) = quad.rect else {
            continue;
        };
        if baseline.targets.contains(&quad.target) {
            continue; // pre-existing, not the addon's
        }
        let Some(owner) = script.target_frame(quad.target) else {
            continue;
        };
        if baseline.frames.contains(&owner) || hangs_off_one_of_ours(script, baseline, owner) {
            report.overlay_quads += 1;
        } else {
            report.own_quads += 1;
        }
        // Unfiltered: an off-screen quad hit-tests to nothing later.
        painted.push(Painted {
            owner,
            point: centre(rect),
        });
        if let Some(name) = script.target_owner_name(quad.target) {
            named.insert(name);
        }
    }
    report.frames = named.into_iter().collect();
    (report, painted)
}

/// The middle of a rect, in the y-up UI space `resolve`, `extract` and `hit_test` share.
fn centre(r: Rect) -> (f32, f32) {
    ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)
}

/// Whether the nearest pre-existing ancestor of `frame` is nested inside a window (an overlay)
/// rather than top-level (the addon's own window). Every window hangs off `UIParent`, so merely
/// reaching a pre-existing frame proves nothing; a detached tree is the addon's own.
fn hangs_off_one_of_ours(script: &UiScript, baseline: &RenderBaseline, frame: FrameHandle) -> bool {
    let mut cursor = Some(frame);
    while let Some(fh) = cursor {
        if baseline.frames.contains(&fh) {
            // A pre-existing frame with no parent is the screen root, not another window.
            return script.frame_parent(fh).is_some();
        }
        cursor = script.frame_parent(fh);
    }
    false
}
