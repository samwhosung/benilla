//! **The headless hover probe** — aim the mouseover pick from a screen point when
//! the window has no OS cursor of its own, and say what the pick found.
//!
//! ## Why this exists
//!
//! An automated run **cannot hover anything**. The rig's probe window (640×360, corner-parked,
//! always-on-top) never receives the OS cursor: `Window::cursor_position()` stays `None` through a
//! `CGWarpMouseCursorPosition` *and* through HID-level `mouseMoved` events posted into its content
//! rect, so [`super::hover::update_hovered_object`] returns before the pick on every frame. The
//! consequence is not subtle — every report of the shape *"this object shows no tooltip"* has had to
//! be settled by the director hovering it and reading the card back to us, because the pick was
//! reachable only by a person with a mouse (2248 recorded that gap; this closes it).
//!
//! ## What it does
//!
//! `WOW_HOVER_PROBE` names where to aim, in the window's own cursor space (logical px, y-down):
//!
//! | value | aim |
//! |---|---|
//! | `centre` / `center` | the window's middle |
//! | `<x>,<y>` | that point |
//! | `sweep` | a 7×5 grid across the middle half of the window, one point per frame, cycling |
//! | `lock` | sweep until something is picked, then HOLD that object for the rest of the run |
//!
//! `sweep` is the one that makes a rig run useful without a camera: standing a body in front of a
//! known object and asking *"what is anywhere near the middle of the screen"* answers the question
//! a fixed point can only answer if the aim was already right. It is a probe, not a camera search —
//! the grid is fixed, bounded and the same every run.
//!
//! **`lock` is what a resting pointer looks like, and `sweep` cannot stand in for it** (2255). A
//! sweep re-enters the object every cycle, and a *re-enter* is precisely what rebuilds the
//! mouseover plate — so a whole class of defect, the one where a plate that is already up never
//! comes back after something else takes the tooltip, is invisible to a sweep by construction. It
//! is also what a fixed `x,y` cannot do: the camera settles over the first seconds of a rig run and
//! the object slides across the screen, so a hand-aimed point that hit at 30 s misses at 60 s.
//! `lock` holds the hit POINT IN THE OBJECT'S OWN FRAME and re-projects it every frame, so the aim
//! follows the object through camera drift and through the object's own motion. It never releases:
//! that is the point — it stands for a pointer a person put down and left there.
//!
//! **Armed only when the window reports no cursor**, so an attended run is never overridden: a
//! person's pointer always wins, and leaving the variable set costs a player nothing.

use bevy::prelude::*;
use bevy::window::Window;

/// The aim, parsed once per process.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Aim {
    Centre,
    At(i32, i32),
    Sweep,
    Lock,
}

fn aim() -> Option<Aim> {
    static AIM: std::sync::OnceLock<Option<Aim>> = std::sync::OnceLock::new();
    *AIM.get_or_init(|| {
        let raw = std::env::var("WOW_HOVER_PROBE").ok()?;
        let v = raw.trim();
        match v {
            "centre" | "center" => Some(Aim::Centre),
            "sweep" => Some(Aim::Sweep),
            "lock" => Some(Aim::Lock),
            _ => match v.split_once(',') {
                Some((x, y)) => match (x.trim().parse(), y.trim().parse()) {
                    (Ok(x), Ok(y)) => Some(Aim::At(x, y)),
                    _ => {
                        warn!(
                            "hover probe: WOW_HOVER_PROBE={v:?} is not `centre`, `sweep`, \
                             `lock` or `x,y`"
                        );
                        None
                    }
                },
                None => {
                    warn!(
                        "hover probe: WOW_HOVER_PROBE={v:?} is not `centre`, `sweep`, `lock` or \
                         `x,y`"
                    );
                    None
                }
            },
        }
    })
}

/// Is the probe armed at all? (Cheap enough to ask per frame — one `OnceLock` read.)
pub(super) fn armed() -> bool {
    aim().is_some()
}

/// The point to pick from, given the window and a frame counter — `None` when the probe is not
/// armed. The caller uses it **only** where the real cursor is absent.
pub(super) fn point(window: &Window, frame: u64) -> Option<Vec2> {
    let (w, h) = (window.width(), window.height());
    match aim()? {
        Aim::Centre => Some(Vec2::new(w / 2.0, h / 2.0)),
        Aim::At(x, y) => Some(Vec2::new(x as f32, y as f32)),
        // The grid: 7 columns × 5 rows over the middle half, so the edges of the frame (chrome,
        // sky, the player's own back) are never the answer. One cell per frame, cycling — at frame
        // rate the whole grid is covered ~2× a second. `lock` searches with the same grid; once it
        // holds an object [`LockedAim::point`] answers ahead of this and the grid stops mattering.
        Aim::Sweep | Aim::Lock => {
            const COLS: u64 = 7;
            const ROWS: u64 = 5;
            let cell = frame % (COLS * ROWS);
            let (cx, cy) = (cell % COLS, cell / COLS);
            let fx = 0.25 + 0.5 * (cx as f32 / (COLS - 1) as f32);
            let fy = 0.25 + 0.5 * (cy as f32 / (ROWS - 1) as f32);
            Some(Vec2::new(w * fx, h * fy))
        }
    }
}

/// Does the aim HOLD its first pick? (`WOW_HOVER_PROBE=lock`.)
pub(super) fn locks() -> bool {
    matches!(aim(), Some(Aim::Lock))
}

/// **This frame's aim**, published by the pick and read by everything else that needs to know
/// where the probe is pointing: the UI mouse feed (so `PointerOverUi` rises and falls over a panel
/// exactly as it does for a person) and the tooltip's cursor-seated arm.
///
/// It is a process-global rather than a resource for two reasons. The probe already is one — the
/// env var behind [`aim`] — and its three readers sit in three different systems that are each at
/// or near Bevy's parameter ceiling, so a resource would cost a bundle refactor apiece for a debug
/// instrument with exactly one writer.
///
/// **One aim, published once per frame, is the correctness claim and not a convenience.** Before
/// 2255 each reader called [`point`] again for itself, and the tooltip's call passed frame `0` —
/// so on a `sweep` the plate was seated at grid cell 0 while the pick was aiming at cell N, and the
/// two disagreed about where the pointer was on 34 frames out of every 35.
static AIM_NOW: std::sync::Mutex<Option<Vec2>> = std::sync::Mutex::new(None);

/// Publish this frame's aim. Called by the pick BEFORE its own gates: a frame that skipped the
/// pick must still report a pointer, or the UI feed would take the pointer off the UI and unlatch
/// the very gate that skipped it.
pub(super) fn publish(point: Option<Vec2>) {
    *AIM_NOW.lock().unwrap_or_else(|e| e.into_inner()) = point;
}

/// This frame's aim, as published. `None` when the probe is not armed.
pub(super) fn now() -> Option<Vec2> {
    *AIM_NOW.lock().unwrap_or_else(|e| e.into_inner())
}

/// `lock` mode's held target: the part the probe first picked, and the point it hit **in that
/// part's own frame**. Re-projected every frame, so the aim follows the object rather than going
/// stale the moment the camera settles or the object moves.
#[derive(Default)]
pub(super) struct LockedAim {
    held: Option<(Entity, Vec3)>,
}

impl LockedAim {
    /// Where the held point is on screen now, or `None` while nothing is held (the grid still
    /// searches) or while it has gone off-camera (the grid searches again).
    pub(super) fn point(
        &self,
        camera: &Camera,
        cam_tf: &GlobalTransform,
        transform_of: impl Fn(Entity) -> Option<GlobalTransform>,
    ) -> Option<Vec2> {
        let (part, local) = self.held?;
        let gt = transform_of(part)?;
        camera
            .world_to_viewport(cam_tf, gt.transform_point(local))
            .ok()
    }

    /// Hold this hit — **once**. A lock that never releases is the whole point; re-locking onto
    /// whatever happened to be picked later would make the aim wander exactly like the sweep it
    /// exists to replace.
    ///
    /// Deliberately does **not** ask [`locks`] itself: the caller gates on the mode (a `sweep` that
    /// held its first hit would stop sweeping), and keeping this half pure is what lets a test
    /// drive it without an env var behind a `OnceLock`.
    pub(super) fn hold(&mut self, part: Entity, gt: &GlobalTransform, hit_world: Vec3) {
        if self.held.is_some() {
            return;
        }
        let local = gt.affine().inverse().transform_point3(hit_world);
        info!("hover probe: LOCKED on {part} at its own {local:?} — the aim holds here now");
        self.held = Some((part, local));
    }
}

/// One line per *change*, so a stationary probe does not flood the log: what the pick found at the
/// aim point, and every term of the GameObject tooltip ladder the card shows a person (2248) —
/// eligibility, the published mouseover, and the ask-once template the plate needs.
#[derive(Default)]
pub(super) struct ProbeReport {
    last: Option<String>,
}

impl ProbeReport {
    pub(super) fn say(&mut self, line: String) {
        if self.last.as_deref() == Some(line.as_str()) {
            return;
        }
        info!("hover probe: {line}");
        self.last = Some(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid stays inside the middle half on both axes — the property that keeps a sweep off the
    /// frame's edges, where the answer is always sky or the player's own back.
    #[test]
    fn the_sweep_grid_stays_in_the_middle_half() {
        let w = 800.0_f32;
        let h = 600.0_f32;
        for cell in 0..35_u64 {
            let (cx, cy) = (cell % 7, cell / 7);
            let fx = 0.25 + 0.5 * (cx as f32 / 6.0);
            let fy = 0.25 + 0.5 * (cy as f32 / 4.0);
            let (x, y) = (w * fx, h * fy);
            assert!((w * 0.25..=w * 0.75).contains(&x), "col {cx} at {x}");
            assert!((h * 0.25..=h * 0.75).contains(&y), "row {cy} at {y}");
        }
    }

    /// **The lock holds its FIRST target and keeps it**, and holds it in the object's own frame —
    /// the two properties that make a locked aim stand for a resting pointer rather than a slowly
    /// wandering one.
    #[test]
    fn the_lock_holds_its_first_target_in_the_objects_own_frame() {
        let mut l = LockedAim::default();
        let first = Entity::from_raw_u32(1).expect("a valid test entity id");
        let later = Entity::from_raw_u32(2).expect("a valid test entity id");
        // A part standing 10 yd east; the ray landed 2 yd up its face.
        let gt = GlobalTransform::from_translation(Vec3::new(10.0, 0.0, 0.0));
        l.hold(first, &gt, Vec3::new(10.0, 2.0, 0.0));
        assert_eq!(
            l.held,
            Some((first, Vec3::new(0.0, 2.0, 0.0))),
            "the hit is stored relative to the part, not in world space"
        );
        l.hold(later, &gt, Vec3::ZERO);
        assert_eq!(
            l.held.map(|(e, _)| e),
            Some(first),
            "a later pick never steals the lock"
        );
    }

    /// Nothing held is nothing aimed — the search half keeps running until the first hit.
    #[test]
    fn an_unheld_lock_aims_at_nothing() {
        let l = LockedAim::default();
        let cam = Camera::default();
        assert!(l
            .point(&cam, &GlobalTransform::IDENTITY, |_| None)
            .is_none());
    }

    /// The published aim round-trips, and clearing it is how a frame says "no pointer here".
    #[test]
    fn the_published_aim_round_trips() {
        publish(Some(Vec2::new(3.0, 4.0)));
        assert_eq!(now(), Some(Vec2::new(3.0, 4.0)));
        publish(None);
        assert_eq!(now(), None);
    }

    /// A repeated line is said once — the property that makes the probe readable in a log rather
    /// than 60 identical lines a second.
    #[test]
    fn the_report_only_speaks_on_change() {
        let mut r = ProbeReport::default();
        r.say("a".into());
        assert_eq!(r.last.as_deref(), Some("a"));
        r.say("a".into());
        assert_eq!(r.last.as_deref(), Some("a"));
        r.say("b".into());
        assert_eq!(r.last.as_deref(), Some("b"));
    }
}
