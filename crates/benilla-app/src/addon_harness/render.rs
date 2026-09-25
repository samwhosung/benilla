//! **The render column — "did this addon put anything on screen?"**
//!
//! Every other column in [`super`] is load- or event-based: they ask whether something *raised*.
//! None of them could ask whether anything was **drawn**, and that gap is not theoretical — the
//! director installed Bagnon, opened their bags, and saw a title and a gold line with **no bag
//! slots at all**, while the survey scored it `loaded, missing=0, session=ok, probe=ok`. The cause
//! raised nothing anywhere (`crate::ui_script::bagnon_render_tests` is the reproduction), so no
//! error-shaped instrument could ever have found it.
//!
//! ## The oracle: a handle diff, not a name and not a total
//!
//! [`RenderBaseline::of`] snapshots every live draw target *before* the addon runs a line; anything
//! new afterwards was created by the addon. Both obvious alternatives are wrong, and wrong in ways
//! this arc has already been bitten by:
//!
//! - **A with/without quad-count diff** reads zero for an addon that *replaces* rather than adds.
//!   Bagnon takes the bags over: our own `BenillaBagFrame` stops drawing as Bagnon starts, and the
//!   total barely moves whether Bagnon painted sixteen slots or none. (Measured: 16 either way, in
//!   the first draft of the reproduction test.)
//! - **A name-prefix match** cannot see an anonymous frame, and `!OmniCC`'s entire visible output
//!   hangs off `CreateFrame("Frame", nil, cooldown:GetParent())` — no name at all.
//!
//! Handles are generational, so a destroyed-and-reused slot can never be mistaken for a survivor.
//!
//! ## Three answers, because the interesting one is the middle
//!
//! [`Drew`] separates an addon that built **its own window** from one that **painted over ours**,
//! and that split is usually the most useful thing here: it points straight at the cause. The two
//! addons the director has verified by eye land on opposite sides of it — `!OmniCC` works and is
//! an [`Drew::Overlay`], Bagnon has to build its own slot buttons and was a [`Drew::Nothing`] — and
//! a check that could only see one of the two shapes would have scored the working addon zero.
//!
//! ## What it over- and under-reports, stated rather than hidden
//!
//! - **Over-reports**: any quad from a new widget counts, including one drawn off-screen or under
//!   another window. "Something reached the render list" is not "the player can see it"; the
//!   director's eye is still the judge of that (`docs/METHOD.md` §7). Over-reporting is the deliberate
//!   direction — a silent under-report is the failure mode this whole column exists to end.
//! - **Under-reports** in one known shape: an addon that changes an *existing* widget in place —
//!   `SetTexture` on one of our regions, `SetBackdrop` on one of our frames — creates no new
//!   handle and scores [`Drew::Nothing`]. Catching that needs a per-quad content diff, which is a
//!   different instrument; it is named here so nobody reads a `nothing` row as proof.
//! - **The world is minimal**, exactly as [`super::drive_session_start`] says: no real bank, no
//!   real raid, no real quests. An addon whose window only opens on state we do not simulate draws
//!   nothing here and would draw in play.

use std::collections::{BTreeSet, HashSet};

use benilla_ui::layout::Rect;
use benilla_ui::order::ZTarget;
use benilla_ui::script::{QuadContent, UiScript};
use benilla_ui::widget::FrameHandle;

/// Every widget that existed before an addon ran — the set the render probe diffs against.
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

    /// Did this frame exist before the addon ran a line?
    ///
    /// The one question [`super::use_probe`] asks of the baseline: a pointer event is only the
    /// addon's to be judged by when the frame that captures it is one the addon itself created.
    pub(super) fn is_pre_existing(&self, frame: FrameHandle) -> bool {
        self.frames.contains(&frame)
    }
}

/// One place on screen the addon **painted** — a candidate for [`super::use_probe`] to point at.
///
/// Emitted by the same walk that attributes the render column, and that is deliberate: "which
/// pixels are this addon's" is one question with one answer, and computing it twice is how two
/// instruments start disagreeing about the same addon.
pub(super) struct Painted {
    /// The frame the quad was charged to. Only used to **dedupe** candidates — sixteen quads on
    /// one button should not spend sixteen hit-tests — never to decide what gets driven; the
    /// hit-test does that, because the frame that owns a painted region is often not the frame
    /// that would actually eat a click on it.
    pub(super) owner: FrameHandle,
    /// The centre of the painted rect: where a player's cursor would be to be "on" this pixel.
    pub(super) point: (f32, f32),
}

/// What an addon put on screen.
///
/// Ordered by how much of the screen the addon owns, so a `>` comparison reads the way it looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Drew {
    /// Not one painting quad came from anything this addon created. Bagnon's row before the fix,
    /// and the answer this column was built to be able to give.
    #[default]
    Nothing,
    /// It painted, but only from widgets hanging off frames that already existed — an overlay on
    /// somebody else's window. `!OmniCC`'s countdown text is the reference case: an anonymous
    /// `Frame` + `FontString` parented to one of our action buttons' cooldowns.
    Overlay,
    /// It painted from a window of its own — a frame tree rooted outside everything that existed
    /// before it. Bagnon's inventory window, once it works.
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
    /// **Every** distinct named frame, not a sample — [`MAX_NAMED_FRAMES`] bounds only what is
    /// PRINTED, because a caller that asks "did it draw an item slot?" cannot answer that from a
    /// truncated list, and one of ours silently could not.
    ///
    /// The distinct named frames those quads were charged to, nearest-named-ancestor per quad, so
    /// a `nothing`/`own` row can be read back to a place in the addon's own code. Capped, sorted.
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

/// How many named frames a row PRINTS. Enough to name the window; not enough to be a dump.
///
/// **This bounds the display, not the collection, and that distinction is the whole of 1242.** It
/// used to stop the `named` set filling at 6 — so a row kept the first six names *encountered* and
/// then rendered them through a `BTreeSet`, i.e. sorted. The output therefore read as an
/// alphabetical list of what an addon drew while actually being an arbitrary six of N, and nothing
/// said so.
///
/// That is not hypothetical damage. `render_tests`'
/// `the_directors_two_verified_addons_come_out_on_opposite_sides` asserts Bagnon draws frames named
/// `BagnonItem*` — the director's original complaint. It passed only because `BagnonItem1` happened
/// to be among the first six quads attributed. Seating the player's PURSE in the session fixture
/// added `BagnonMoneyFrameSilverButton` to the same window, which took the sixth slot, evicted
/// `BagnonItem1`, and made the test assert — correctly, against the data it was given — that Bagnon
/// draws no item slots at all. The addon had not changed.
///
/// So the set now collects everything and the cap applies where the constraint actually is: the
/// printed line. 1242's rule, in the second instrument to break it.
pub const MAX_NAMED_FRAMES: usize = 6;

/// Drive the entry points that make a UI **visible**, then attribute every painting quad.
///
/// **This is a different probe from [`super::drive_ui_probe`], not a duplicate of it**, and the
/// difference is the whole point: that one *toggles* — open then close — because what it measures
/// is whether an override raises on both paths. A render read after it sees a closed window. This
/// one only ever **opens**, and leaves everything up, because what it measures is what is on the
/// screen at the end.
///
/// The entry points are chosen because a corpus addon is measured to replace or hook each:
///
/// | driven | why |
/// |---|---|
/// | `OpenBackpack()` | Bagnon replaces it — and with an *open*, not a toggle, unlike its `ToggleBackpack`/`OpenAllBags` |
/// | `CooldownFrame_SetTimer` on a real button | `!OmniCC` hooks exactly this global; its text exists only while a cooldown runs |
/// | `ActionButton_Update` | zBar, zBarEx, CT_BarMod |
/// | a shown `GameTooltip` | the hover class — left SHOWN, where the UI probe hides it |
///
/// Then ten ticks, because a great deal of addon painting happens on the first `OnUpdate` —
/// OmniCC's countdown text is written there and nowhere else, so a probe that never ticked would
/// score our one director-verified working addon as blank.
///
/// Returns the report **and the places the addon painted**, in painter order, for
/// [`super::use_probe`] to point at. Nothing else in the survey knows which pixels are the addon's,
/// and it should stay that way: one attribution, one answer.
pub(super) fn measure_render(
    script: &mut UiScript,
    baseline: &RenderBaseline,
) -> (RenderReport, Vec<Painted>) {
    // A REAL ACTION on slot 1, with a real icon and a running cooldown.
    //
    // Not decoration — without it this column scored `!OmniCC` as blank, and `!OmniCC` is on the
    // director's screen. Its `OnUpdate` writes the countdown only `if floor(remain + 0.5) > 0 and
    // this.icon:IsVisible()`, and on an empty bar `ActionButton1Icon` is hidden, so the text frame
    // is created, shown, and never given a single character. An empty action bar is a state a
    // level-60 session does not present; `seat_a_session`'s argument, one slot wide.
    //
    // Seated HERE rather than in `seat_a_session` on purpose (1225's rule): this runs after every
    // other column has been read, so no number beside this one can be perturbed by it.
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
    // A REAL BACKPACK, for the same reason. `GetContainerNumSlots(0)` answers 16 for every
    // character in the game; in a VM where it answers 0 a bag addon correctly draws a window with
    // no slots in it — which is *exactly* the picture this column was built to be able to catch,
    // so leaving it unseated would make the column blind to its own founding case. Sixteen empty
    // slots, no items: an addon that paints a slot paints it whether or not something is in it.
    script.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    // Guarded on each global being a FUNCTION and each raise swallowed: a raise here is
    // `probe_errors`' column to report, and letting one abort the run would silently zero every
    // measurement after it (the mis-attribution 1209 was written about).
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
        // A frame's own slot paints NOTHING (`ui_script::extract`'s converter drops it outright);
        // a Backdrop, Texture, Text, Minimap or Cooldown is pixels. Counting the frame slot would
        // score every addon that merely *creates* a frame as having drawn, which is the exact
        // false pass this column exists to stop.
        if matches!(quad.content, QuadContent::Frame) {
            continue;
        }
        // Under-constrained widgets never reach the screen either. Bound here rather than merely
        // tested, because the rect is also WHERE the addon painted — the point `use_probe` aims at.
        let Some(rect) = quad.rect else {
            continue;
        };
        if baseline.targets.contains(&quad.target) {
            continue; // ours, drawing as it always did
        }
        let Some(owner) = script.target_frame(quad.target) else {
            continue;
        };
        if baseline.frames.contains(&owner) || hangs_off_one_of_ours(script, baseline, owner) {
            report.overlay_quads += 1;
        } else {
            report.own_quads += 1;
        }
        // The same quad, kept as a place to point at. An off-screen or zero-area one simply
        // hit-tests to nothing later, which is the honest answer for it — no filtering here, or
        // the two columns would disagree about which pixels are the addon's.
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

/// The middle of a rect, in the y-up UI space `resolve`/`extract`/`hit_test` all share.
fn centre(r: Rect) -> (f32, f32) {
    ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)
}

/// Does this new frame hang off one of **our widgets**, rather than off the screen?
///
/// The overlay/own split, and it needs more care than "did it reach a pre-existing frame", because
/// *everything* does: `UIParent` is where every window in the game is parented, ours and theirs
/// alike, so that test calls every addon an overlay (it did — this function's first draft failed
/// its own fixture). The seam is one step further in: walk to the nearest ancestor that existed
/// before the addon ran, and ask whether **it** is a top-level frame.
///
/// - nearest pre-existing ancestor is top-level (`UIParent`, `WorldFrame`, the screen root) → the
///   addon put a window on the screen. Bagnon.
/// - nearest pre-existing ancestor is *nested* inside one of our windows → the addon decorated
///   something of ours. `!OmniCC`'s anonymous text frame, whose parent is an action button's
///   cooldown.
/// - no pre-existing ancestor at all (a detached tree) → its own, by the same reading.
///
/// The walk is bounded by the tree's own depth.
fn hangs_off_one_of_ours(script: &UiScript, baseline: &RenderBaseline, frame: FrameHandle) -> bool {
    let mut cursor = Some(frame);
    while let Some(fh) = cursor {
        if baseline.frames.contains(&fh) {
            // `UIParent` and friends carry no parent of their own — reaching one of those means
            // the addon reached the *screen*, which is not somebody else's window.
            return script.frame_parent(fh).is_some();
        }
        cursor = script.frame_parent(fh);
    }
    false
}
