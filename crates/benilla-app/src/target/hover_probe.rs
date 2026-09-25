//! The headless hover probe: aims the mouseover pick from a screen point when the window has no OS
//! cursor, and reports what the pick found. An unattended run's window never receives the OS
//! cursor, so without it [`super::hover::update_hovered_object`] never reaches the pick.
//!
//! `WOW_HOVER_PROBE` names the aim, in the window's cursor space (logical px, y-down):
//!
//! | value | aim |
//! |---|---|
//! | `centre` / `center` | the window's middle |
//! | `<x>,<y>` | that point |
//! | `sweep` | a fixed 7×5 grid over the middle half of the window, one point per frame, cycling |
//! | `lock` | sweep until something is picked, then hold that object for the rest of the run |
//!
//! `lock` stands for a resting pointer: it holds the hit point in the object's own frame and
//! re-projects it every frame, so it follows camera drift and the object's motion, and it never
//! releases. A sweep re-enters its object every cycle, and a re-enter rebuilds the mouseover plate,
//! so only `lock` can show a defect in a plate that is already up.
//!
//! Armed only while the window reports no cursor, so a person's pointer always wins.

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

/// Whether the probe is armed: one `OnceLock` read, cheap per frame.
pub(super) fn armed() -> bool {
    aim().is_some()
}

/// The point to pick from this frame; the caller uses it only where the real cursor is absent.
pub(super) fn point(window: &Window, frame: u64) -> Option<Vec2> {
    let (w, h) = (window.width(), window.height());
    match aim()? {
        Aim::Centre => Some(Vec2::new(w / 2.0, h / 2.0)),
        Aim::At(x, y) => Some(Vec2::new(x as f32, y as f32)),
        // 7 × 5 cells over the middle half, off the frame's edges (chrome, sky, the player's own
        // back), one per frame. Once `lock` holds an object, `LockedAim::point` answers first.
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

/// Whether the aim holds its first pick (`lock`).
pub(super) fn locks() -> bool {
    matches!(aim(), Some(Aim::Lock))
}

/// This frame's aim, published once by the pick so the UI mouse feed and the tooltip's
/// cursor-seated arm agree with it about where the pointer is. A process global, like the env var
/// behind it: its readers are systems already at Bevy's parameter ceiling.
static AIM_NOW: std::sync::Mutex<Option<Vec2>> = std::sync::Mutex::new(None);

/// Called by the pick before its own gates: a frame that skipped the pick still reports a pointer,
/// or the UI feed would drop it and unlatch the gate that skipped it.
pub(super) fn publish(point: Option<Vec2>) {
    *AIM_NOW.lock().unwrap_or_else(|e| e.into_inner()) = point;
}

/// This frame's published aim.
pub(super) fn now() -> Option<Vec2> {
    *AIM_NOW.lock().unwrap_or_else(|e| e.into_inner())
}

/// `lock`'s held target: the part first picked, and the hit point in that part's own frame.
#[derive(Default)]
pub(super) struct LockedAim {
    held: Option<(Entity, Vec3)>,
}

impl LockedAim {
    /// The held point on screen now; `None` while nothing is held or it is off-camera, and the grid
    /// searches.
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

    /// Holds the first hit and never another. The caller gates on the mode, which keeps this
    /// testable without the env var.
    pub(super) fn hold(&mut self, part: Entity, gt: &GlobalTransform, hit_world: Vec3) {
        if self.held.is_some() {
            return;
        }
        let local = gt.affine().inverse().transform_point3(hit_world);
        info!("hover probe: LOCKED on {part} at its own {local:?} — the aim holds here now");
        self.held = Some((part, local));
    }
}

/// One log line per change: what the pick found, and the GameObject tooltip ladder's terms
/// (eligibility, the published mouseover, the ask-once template).
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

    #[test]
    fn an_unheld_lock_aims_at_nothing() {
        let l = LockedAim::default();
        let cam = Camera::default();
        assert!(l
            .point(&cam, &GlobalTransform::IDENTITY, |_| None)
            .is_none());
    }

    #[test]
    fn the_published_aim_round_trips() {
        publish(Some(Vec2::new(3.0, 4.0)));
        assert_eq!(now(), Some(Vec2::new(3.0, 4.0)));
        publish(None);
        assert_eq!(now(), None);
    }

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
