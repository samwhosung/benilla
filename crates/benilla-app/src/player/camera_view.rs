//! The **five camera views** — `SetView` / `SaveView` / `ResetView` / `NextView` / `PrevView`, and
//! `FlipCameraYaw`. The engine half of [`benilla_ui::script::CameraViewRequest`]; the Lua half (and
//! the argument ABI) is `benilla-ui`'s `script::camera_view`.
//!
//! [`super::camera_saved`]'s neighbour, and its complement: that file remembers the **one live
//! pose** per character; this one remembers the **five named poses** the player can jump between.
//! The reference splits them exactly the same way and for the same reason — one is where you left
//! the camera, the other is where you decided it should be able to go.
//!
//! # The five views, byte-real
//!
//! The reference's camera ctor reads the `cameraView` CVar and indexes a five-row table of default
//! *strings* at `0x84f488` (stride 12, three `const char*` per row — distance, pitch, yaw), with
//! the row names at `0x84f848`. Read straight out of `WoW.exe` 5875 (decision 1138 §2 records the
//! first two columns; the third is dumped here for the first time):
//!
//! | `SetView` arg | internal | name | distance | pitch | yaw |
//! |---|---|---|---|---|---|
//! | 1 | 0 | `FIRST_PERSON`   | `0.0`   | `0.0`  | `0.0` |
//! | 2 | 1 | `THIRD_PERSON_A` | `5.55`  | `10.0` | `0.0` |
//! | 3 | 2 | `THIRD_PERSON_B` | `5.55`  | `20.0` | `0.0` |
//! | 4 | 3 | `THIRD_PERSON_C` | `13.88` | `30.0` | `0.0` |
//! | 5 | 4 | `THIRD_PERSON_D` | `13.88` | `10.0` | `0.0` |
//!
//! The pitch column is **degrees, positive = looking DOWN** — the reference's own convention, which
//! is the opposite of [`FlyCam::pitch`]'s. Decision 1138 is the whole story; the conversion is
//! [`super::camera_saved::pitch_from_file`] and this module does not grow a second one.
//!
//! View 1 is `distance 0.0`, and that is **reachable, not a rounding of "very close"**:
//! [`CAM_DIST_MIN`] is `0.0`, the eye sits on the framing pivot (inside the head) and the avatar
//! fades out through [`benilla_world::model_fade::self_model_fade_alpha`]. `SetView(1)` is real
//! first person, not a clamp to some near stop.
//!
//! # Persistence — the reference's own fifteen CVars
//!
//! 1138 §2 recorded that the saved views survive a restart as archived `config.wtf` CVars and left
//! the name→view mapping unpinned. It is pinned now, at `0x50b9b0`'s 5x3 registration loop, and the
//! names are the reference's:
//!
//! ```text
//! 50b9c1  SStrCopy(buf, 0x84fdc0)              ; "camera"
//! 50b9d3  SStrCat (buf, [esi+0x84f468])        ; "Distance" | "Pitch" | "Yaw"   (inner, esi 0..12)
//! 50b9e1  SStrCat (buf, [ebx])                 ; ""  | "A" | "B" | "C" | "D"    (outer, per view)
//! 50b9ec  default = [esi + edi + 0x84f488]     ; the SAME table as the ctor's, 12·view + 4·field
//! 50ba07  record  -> [esi + edi + 0xbe0f7c]
//! ```
//!
//! → `cameraDistance`/`cameraPitch`/`cameraYaw` for view 0, and `…A`/`…B`/`…C`/`…D` for views 1–4;
//! the active view is `cameraView` (`0x84fdc8`, registrar default `"1"` — an *internal* index, so
//! the shipped view is `THIRD_PERSON_A`, i.e. Lua `SetView(2)`). Verified by reading the tables out
//! of the binary, not inferred from the loop shape. benilla registers exactly those sixteen names
//! with exactly those defaults ([`crate::cvars::REGISTERED`], welded by test below) — 0954's CVar
//! store *is* our persistence, so being faithful here costs nothing and buys a `config.toml` a
//! player can read against a real `config.wtf`.
//!
//! **No host knob** — the `lastCharacterIndex`/`checkAddonVersion` posture in [`crate::cvars`]:
//! [`CameraViews`] is the authority, seeded once from the persisted values at startup and writing
//! every change back through `set_cvar_engine` (the minimap-zoom pattern, so the write dirties
//! `config.toml`). A live `SetCVar("cameraPitchB", …)` therefore does *not* reach the in-memory
//! slot — which is the reference's behaviour too: `0x50fb80` loads the slot array from the CVar
//! records at camera construction and nothing re-reads them afterwards.
//!
//! # Three things the reference does that a reader will expect it not to
//!
//! - **`SaveView(1)` and `ResetView(1)` work.** All three indexed entry points take `1..=5`
//!   (`0x50b5d2`/`0x50b626`/`0x50b666`: `test eax,eax; jle` then `cmp eax,5; jg`), and `0x50fa30`
//!   — SaveView's body — has no view-0 gate. 1.12 ships no `SAVEVIEW1`/`RESETVIEW1` *binding*,
//!   which is a `Bindings.xml` decision; a macro reaches what no key can.
//! - **`NextView`/`PrevView` do not wrap.** `0x50faa0` is `eax = view + 1; cmp eax,5; jge <ret>`,
//!   `0x50fac0` is `test eax,eax; jle <ret>; dec eax`. Holding `END` walks 1→2→3→4→5 and stops.
//! - **The saved yaw is an OFFSET from the subject's facing, not a world angle.** The reference's
//!   yaw channel `cam+0xf0` is relative while `cam+0xa4 == 0` and the final view resolver re-adds
//!   the followed unit's facing every frame (`0x50f7f2`); `0x512e90` adds it explicitly on the
//!   absolute-mode leg (`0x512fff`). Ours stores `wrap_pi(cam.yaw − face_yaw)` for the same reason.
//!
//! # The active view is remembered, and not applied at startup
//!
//! [`load_saved_views`] seeds [`CameraViews`] and stops — it never calls [`apply`]. So a
//! `cameraView` of `2` means "`NextView` steps from there", not "open the camera at view 2"; the
//! pose you actually get is the one [`super::camera_saved`] restores, which is where you left the
//! camera. That is not a divergence in *effect*: the reference builds its camera from
//! `cameraView`'s row at construction and then `camera-settings.txt`'s reader lands distance and
//! pitch over the top of it at UI load (1138), so the remembered pose wins there too. It is stated
//! because the two modules are neighbours and a reader will otherwise expect them to fight.
//!
//! # Two deliberate divergences, both stated
//!
//! - **`ResetView` writes the CVars back to their defaults; the reference does not.** `0x50fae0`
//!   re-parses the default strings into the slot array and re-applies the view, and touches no
//!   CVar — so in the real client a reset comes *back* at the next launch, out of the value
//!   `SaveView` archived. Here the CVar store is the persistence, so a reset that does not
//!   persist is a bug rather than a quirk worth aping; writing the default string verbatim also
//!   makes [`crate::cvars`]'s diff-shaped file drop the key entirely.
//! - **`SetView` glides the distance and snaps pitch and yaw.** The reference arms all three of its
//!   smooth channels (`0x513189`/`0x5131a3`/`0x5131b8`, on `cameraViewBlendStyle`'s default `1`).
//!   benilla's rig has exactly one such channel — the wheel-zoom glide at `cameraDistanceMoveSpeed`
//!   — and setting `target_distance` rides it for free; there is no pitch or yaw channel to arm, so
//!   those land immediately. A second and a third channel is a real piece of work and a *look*
//!   call; it is named here rather than half-built.

use bevy::prelude::*;

use benilla_ui::script::{CameraViewRequest, UiScript, CAMERA_VIEW_COUNT};
use benilla_world::view::WorldCamera;

use crate::creature_anim::wrap_pi;

use super::camera::{CameraControl, FlyCam, CAM_DIST_MAX, CAM_DIST_MIN, CAM_PITCH_LIMIT};
use super::camera_saved::{pitch_from_file, pitch_to_file};
use super::Player;

/// How many views there are — the reference's default table `0x84f488` is five rows, and every
/// entry point is bounded by it. Named through the UI crate's constant so the two ends of the
/// queue cannot disagree about the range the Lua side already checked.
const VIEW_COUNT: usize = CAMERA_VIEW_COUNT as usize;

/// The registered CVar name of the active view — the reference's own (`0x84fdc8`), holding the
/// **internal** index as `"%d"` (`0x512ef1`, format `0x835154`).
pub(crate) const CVAR_ACTIVE_VIEW: &str = "cameraView";

/// The fifteen saved-view CVar names, `[view][field]` with field = distance, pitch, yaw —
/// `"camera" + {"Distance","Pitch","Yaw"} + {"","A","B","C","D"}`, composed at `0x50b9b0` (see the
/// module header). Written out rather than composed so [`crate::cvars::REGISTERED`] can hold the
/// same `&'static str`s and a test can weld the two tables together.
pub(crate) const VIEW_CVARS: [[&str; 3]; VIEW_COUNT] = [
    ["cameraDistance", "cameraPitch", "cameraYaw"],
    ["cameraDistanceA", "cameraPitchA", "cameraYawA"],
    ["cameraDistanceB", "cameraPitchB", "cameraYawB"],
    ["cameraDistanceC", "cameraPitchC", "cameraYawC"],
    ["cameraDistanceD", "cameraPitchD", "cameraYawD"],
];

/// The shipped defaults, **as the reference's own strings** — `0x84f488`, read out of `WoW.exe`
/// 5875. Strings, not floats, for two reasons: they are what the binary stores (its ctor calls
/// `SStrToFloat` on them), and [`crate::cvars`]'s file is a diff against the registered default
/// *string*, so a reset can only strip the key by writing this exact text back.
pub(crate) const VIEW_DEFAULTS: [[&str; 3]; VIEW_COUNT] = [
    ["0.0", "0.0", "0.0"],
    ["5.55", "10.0", "0.0"],
    ["5.55", "20.0", "0.0"],
    ["13.88", "30.0", "0.0"],
    ["13.88", "10.0", "0.0"],
];

/// The registrar default of [`CVAR_ACTIVE_VIEW`] — `"1"` (`[0x84f484]`), the *internal* index, so
/// the client opens on `THIRD_PERSON_A`.
pub(crate) const ACTIVE_VIEW_DEFAULT: &str = "1";

/// One view's pose, in **benilla's** units: yards, and radians with [`FlyCam::pitch`]'s
/// positive-is-UP sign. `yaw` is an offset from the subject's facing (see the module header), so a
/// view whose yaw is `0.0` is "directly behind" whichever way the character is pointing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewPose {
    pub(crate) distance: f32,
    pub(crate) pitch: f32,
    pub(crate) yaw: f32,
}

impl ViewPose {
    /// A pose out of the reference's three stored strings — the units the CVars (and the default
    /// table) carry: yards, degrees-positive-down, degrees. Clamped on the way in, at this one
    /// edge, so nothing downstream can be handed an unreachable view: the distance to the zoom
    /// range and the pitch to +-89 deg (the reference's own CVar validators are `0x50b310`'s
    /// `[0, 50]` and `0x50b380`'s +-89 deg; ours are the rig's, which is the tighter, honest pair).
    ///
    /// The **yaw is wrapped rather than range-checked**: the reference's validator is `[0, 360]`
    /// (`0x50b3c0`) while the value its `SaveView` writes is a signed offset, so both encodings are
    /// in the wild and `wrap_pi` reads either.
    fn from_reference(distance: f32, pitch_deg: f32, yaw_deg: f32) -> Self {
        Self {
            distance: distance.clamp(CAM_DIST_MIN, CAM_DIST_MAX),
            // The one bridge, shared with the per-character pose file.
            pitch: pitch_from_file(pitch_deg),
            yaw: wrap_pi(yaw_deg.to_radians()),
        }
    }

    /// The three strings to persist, in the reference's own `"%f"` (`0x50f9f3`, format `0x835160`)
    /// — six decimals, which is also what [`super::camera_saved`] writes for the same numbers.
    fn to_reference(self) -> [String; 3] {
        [
            format!("{:.6}", self.distance),
            format!("{:.6}", pitch_to_file(self.pitch)),
            format!("{:.6}", self.yaw.to_degrees()),
        ]
    }

    /// The shipped default for one view, parsed from the reference's own default strings.
    fn shipped(view: usize) -> Self {
        let row = VIEW_DEFAULTS[view];
        let parse = |s: &str| {
            s.parse::<f32>()
                .expect("the reference's default strings are floats")
        };
        Self::from_reference(parse(row[0]), parse(row[1]), parse(row[2]))
    }
}

/// The five views and which one is live — the reference's `[cam+0xb0 .. +0xdc]` slot array and its
/// `[cam+0xac]` index, as a resource.
#[derive(Resource, Debug, PartialEq)]
pub(crate) struct CameraViews {
    views: [ViewPose; VIEW_COUNT],
    /// The live view, `0..VIEW_COUNT` — the reference's `cam+0xac`, persisted as [`CVAR_ACTIVE_VIEW`].
    current: usize,
}

impl Default for CameraViews {
    fn default() -> Self {
        Self {
            views: std::array::from_fn(ViewPose::shipped),
            current: ACTIVE_VIEW_DEFAULT
                .parse::<usize>()
                .expect("the registrar default is an index"),
        }
    }
}

impl CameraViews {
    /// The pose of one view.
    fn pose(&self, view: usize) -> ViewPose {
        self.views[view]
    }

    /// `SetView` — make `view` live and hand back the pose to seat. Returns whether the index
    /// actually moved, which is the reference's own gate on writing [`CVAR_ACTIVE_VIEW`]
    /// (`0x512ee6 cmp [esi+0xac],edi; je`).
    fn set(&mut self, view: usize) -> (ViewPose, bool) {
        let moved = self.current != view;
        self.current = view;
        (self.views[view], moved)
    }

    /// `SaveView` — store a live pose into `view`. The reference reads the camera's three *target*
    /// fields here (`0x50fa30`: `+0x198` distance, `+0x1e0` pitch, `+0x210` yaw), never the live
    /// ones — which is why a save made with your back to a wall keeps your chosen zoom and not the
    /// collision-pulled arm.
    fn save(&mut self, view: usize, pose: ViewPose) {
        self.views[view] = pose;
    }

    /// `ResetView` — put `view` back to its shipped default. `true` when it was the live view, in
    /// which case the reference re-applies it on the spot (`0x50fb5b`) and so do we.
    fn reset(&mut self, view: usize) -> bool {
        self.views[view] = ViewPose::shipped(view);
        self.current == view
    }

    /// `NextView` / `PrevView` — the neighbouring view, or `None` at either end. **No wrap**:
    /// `0x50faa0`'s `cmp eax,5; jge <ret>` and `0x50fac0`'s `test eax,eax; jle <ret>`.
    fn step(&self, forward: bool) -> Option<usize> {
        if forward {
            (self.current + 1 < VIEW_COUNT).then(|| self.current + 1)
        } else {
            self.current.checked_sub(1)
        }
    }
}

/// Startup: seed the five views (and the live index) from `config.toml`.
///
/// Reads the registry ([`crate::cvars::Cvars::get`]) rather than the VM's table for the reason
/// 1622's remembered-character row does: the VM's mirror is a per-VM `Update` seed and does not
/// exist yet, while the registry is a resource from `CvarLoad` onward and outlives every VM
/// (2303). An absent key answers the shipped default, which is every first run.
fn load_saved_views(cvars: Res<crate::cvars::Cvars>, mut views: ResMut<CameraViews>) {
    for (view, slot) in views.views.iter_mut().enumerate() {
        // Each field is independently optional, like the pose file's two keys: an absent or
        // hand-mangled value costs that one number, not the whole view.
        let field = |i: usize| {
            cvars
                .get(VIEW_CVARS[view][i])
                .and_then(|s| s.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        let shipped_row = VIEW_DEFAULTS[view];
        let raw = |i: usize| {
            field(i).unwrap_or_else(|| {
                shipped_row[i]
                    .parse()
                    .expect("the reference's default strings are floats")
            })
        };
        *slot = ViewPose::from_reference(raw(0), raw(1), raw(2));
    }
    if let Some(v) = cvars
        .get(CVAR_ACTIVE_VIEW)
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|v| *v < VIEW_COUNT)
    {
        views.current = v;
    }
}

/// Everything one drained request may write, fetched once (the app's bundled-param convention).
#[derive(bevy::ecs::system::SystemParam)]
struct ViewTargets<'w, 's> {
    views: ResMut<'w, CameraViews>,
    rig: ResMut<'w, CameraControl>,
    cam: Query<'w, 's, &'static mut FlyCam, With<WorldCamera>>,
    player: Res<'w, Player>,
}

/// Per frame: drain the Lua queue and apply each request to the rig.
///
/// Ordered before [`super::control`] so a view taken this frame is the pose this frame's camera
/// seat is computed from, exactly like the pose file's restore.
fn drain_view_requests(
    script: Option<NonSendMut<UiScript>>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    mut targets: ViewTargets,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_camera_view_requests();
    if requests.is_empty() {
        return;
    }
    let Ok(mut cam) = targets.cam.single_mut() else {
        return; // no camera entity — the reference's `0x4818f0` miss, same silent drop
    };
    let face_yaw = targets.player.facing();
    // What to mirror into the CVar table once the batch is applied: the slots a save or a reset
    // moved, and the live index if it changed. Collected rather than written inline because the VM
    // handle is borrowed for the drain.
    let mut write_slots: Vec<usize> = Vec::new();
    let mut write_active = false;

    for request in requests {
        match request {
            CameraViewRequest::Set(n) => {
                write_active |= apply(
                    &mut targets.views,
                    &mut targets.rig,
                    &mut cam,
                    face_yaw,
                    usize::from(n),
                );
            }
            CameraViewRequest::Save(n) => {
                let view = usize::from(n);
                targets.views.save(
                    view,
                    ViewPose {
                        // `target_distance`, never `distance`: the reference saves `cam+0x198`,
                        // the chosen zoom, not the collision-pulled arm (`camera_saved`'s own
                        // reason, one field over).
                        distance: targets.rig.target_distance,
                        pitch: cam.pitch,
                        yaw: wrap_pi(cam.yaw - face_yaw),
                    },
                );
                write_slots.push(view);
            }
            CameraViewRequest::Reset(n) => {
                let view = usize::from(n);
                if targets.views.reset(view) {
                    apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
                write_slots.push(view);
            }
            CameraViewRequest::Next | CameraViewRequest::Prev => {
                let forward = request == CameraViewRequest::Next;
                if let Some(view) = targets.views.step(forward) {
                    write_active |= apply(
                        &mut targets.views,
                        &mut targets.rig,
                        &mut cam,
                        face_yaw,
                        view,
                    );
                }
            }
            CameraViewRequest::FlipYaw(degrees) => {
                // `cam[+0x100] += arg * pi/180` (`0x50b6a0`). **A named divergence**: the
                // reference accumulates into a yaw field of its OWN, added to the resolved view
                // yaw at `0x50f7de` *outside* the follow channel, so its flip survives the camera
                // re-centring behind a moving character. benilla's rig stores one absolute yaw and
                // has no second channel, so the flip lands there and the auto-follow will reel it
                // back in on Smart/Always (never on Never). Adding that second channel touches
                // `seat_camera`, the follow and the right-drag carry, and changes how the view
                // reads — a look call, not a silent one.
                cam.yaw = wrap_pi(cam.yaw + degrees.to_radians());
            }
        }
    }

    // Mirror into the registry so the change dirties `config.toml` and the view survives a
    // restart — a host write (2303: the engine's own value moved, so the table follows and the
    // VM's mirror learns it). The resource stays authoritative either way, so a run with no VM
    // at all (a bare test app) still has working views.
    for view in write_slots {
        // A reset writes the reference's default STRING verbatim, not a re-rendered `%f` of it:
        // `crate::cvars`'s file is a diff against the registered default, and only the exact text
        // makes the key disappear again.
        let reset_to_default = targets.views.pose(view) == ViewPose::shipped(view);
        let rendered = targets.views.pose(view).to_reference();
        for field in 0..3 {
            let value = if reset_to_default {
                VIEW_DEFAULTS[view][field]
            } else {
                rendered[field].as_str()
            };
            cvars.set(VIEW_CVARS[view][field], value);
        }
    }
    if write_active {
        cvars.set(CVAR_ACTIVE_VIEW, &targets.views.current.to_string());
    }
}

/// Seat one view on the rig, and say whether the live index moved (the [`CVAR_ACTIVE_VIEW`] write's
/// own gate).
///
/// Only `target_distance` is written: the rig's wheel glide then walks `distance` there at
/// `cameraDistanceMoveSpeed`, which is the same channel the reference's `SetView` arms for the
/// distance. Pitch and yaw land immediately (see the module header's second divergence).
fn apply(
    views: &mut CameraViews,
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    face_yaw: f32,
    view: usize,
) -> bool {
    let (pose, moved) = views.set(view);
    rig.target_distance = pose.distance;
    cam.pitch = pose.pitch.clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    cam.yaw = wrap_pi(face_yaw + pose.yaw);
    moved
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CameraViews>()
        // After the config fold, for the same reason the camera's own spawn is: the persisted
        // views have to be in the resource before anything can select one.
        .add_systems(Startup, load_saved_views.after(crate::cvars::CvarLoad))
        .add_systems(
            Update,
            drain_view_requests
                // After the tick that queued them (`SetView`/`SaveView` are Lua's), before the
                // control pass that seats the pose.
                .after(crate::ui_script::UiInput)
                .before(super::control)
                // Capture parks the camera itself (`capture::probe_cam`), and `control` is gated
                // off there for the same reason; a queued view must not steal a parked pose.
                .run_if(not(resource_exists::<crate::run_mode::CaptureMode>)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped view lands the pose the reference's own default table carries — the numbers
    /// out of `0x84f488`, through the one pitch bridge, clamped where the rig
    /// clamps. This is the test that would catch a transcription slip in [`VIEW_DEFAULTS`].
    #[test]
    fn each_shipped_view_lands_its_recorded_pose() {
        // (Lua arg, distance yd, reference pitch degrees — positive is looking DOWN.)
        let table = [
            (1, 0.0_f32, 0.0_f32),
            (2, 5.55, 10.0),
            (3, 5.55, 20.0),
            (4, 13.88, 30.0),
            (5, 13.88, 10.0),
        ];
        let views = CameraViews::default();
        for (lua_arg, distance, pitch_deg) in table {
            let pose = views.pose(lua_arg - 1);
            assert_eq!(pose.distance, distance, "SetView({lua_arg}) distance");
            assert!(
                (pose.pitch - pitch_from_file(pitch_deg)).abs() < 1e-6,
                "SetView({lua_arg}) pitch: {} vs {}",
                pose.pitch,
                pitch_from_file(pitch_deg)
            );
            assert_eq!(
                pose.yaw, 0.0,
                "SetView({lua_arg}) yaw is behind the character"
            );
            // The sign 1138 exists for: the reference's positive pitch is OUR negative.
            assert!(
                pose.pitch <= 0.0,
                "a positive saved pitch looks DOWN for us"
            );
        }
    }

    /// **First person is reachable**, and view 1 is it: distance 0.0 survives the clamp because
    /// [`CAM_DIST_MIN`] is `0.0` and the rig seats the eye on the framing pivot there.
    #[test]
    fn view_one_is_real_first_person_not_a_near_stop() {
        assert_eq!(CAM_DIST_MIN, 0.0);
        assert_eq!(CameraViews::default().pose(0).distance, 0.0);
    }

    /// The shipped view is `THIRD_PERSON_A` — `cameraView`'s registrar default `"1"` is an
    /// *internal* index, i.e. Lua `SetView(2)`.
    #[test]
    fn the_shipped_view_is_third_person_a() {
        assert_eq!(CameraViews::default().current, 1);
        assert_eq!(CameraViews::default().pose(1).distance, 5.55);
    }

    /// Save → move the camera → set: the saved pose comes back. Then reset puts the shipped
    /// default back. The round trip goes through the persisted *strings*, so it also pins that
    /// what we write is what we can read.
    #[test]
    fn a_saved_view_round_trips_and_reset_restores_the_default() {
        let mut views = CameraViews::default();
        let saved = ViewPose {
            distance: 12.25,
            pitch: -0.42,
            yaw: 0.75,
        };
        views.save(2, saved);
        // The camera moves somewhere else entirely...
        views.save(
            3,
            ViewPose {
                distance: 1.0,
                pitch: 0.1,
                yaw: -2.0,
            },
        );
        assert_eq!(views.pose(2), saved);

        // ...and through the CVar text, which is the only form that survives a restart.
        let [d, p, y] = saved.to_reference();
        let reloaded =
            ViewPose::from_reference(d.parse().unwrap(), p.parse().unwrap(), y.parse().unwrap());
        assert!((reloaded.distance - saved.distance).abs() < 1e-4);
        assert!((reloaded.pitch - saved.pitch).abs() < 1e-4);
        assert!((reloaded.yaw - saved.yaw).abs() < 1e-4);

        assert!(!views.reset(2), "view 2 was not the live one");
        assert_eq!(views.pose(2), ViewPose::shipped(2));
        // Resetting the LIVE view reports so — the reference re-applies it on the spot.
        views.set(3);
        assert!(views.reset(3));
    }

    /// **`SaveView(1)`/`ResetView(1)` are real** — the engine's range is `1..=5` for all three
    /// indexed entry points (`0x50b5d2`/`0x50b626`/`0x50b666`), and `0x50fa30` has no view-0 gate.
    /// Only the *binding* table stops at 2; a macro does not.
    #[test]
    fn view_one_is_saveable_and_resettable() {
        let mut views = CameraViews::default();
        let pose = ViewPose {
            distance: 3.0,
            pitch: -0.2,
            yaw: 0.0,
        };
        views.save(0, pose);
        assert_eq!(views.pose(0), pose);
        views.reset(0);
        assert_eq!(views.pose(0), ViewPose::shipped(0));
        assert_eq!(views.pose(0).distance, 0.0);
    }

    /// **Next/Prev clamp; they do not wrap** — `0x50faa0`'s `jge` and `0x50fac0`'s `jle` are hard
    /// stops. The walk is 1→2→3→4→5 and then nothing.
    #[test]
    fn next_and_prev_stop_at_the_ends() {
        let mut views = CameraViews::default();
        views.set(0);
        assert_eq!(views.step(false), None, "PrevView at view 1 is a no-op");
        for expected in 1..VIEW_COUNT {
            let next = views.step(true).expect("a view above this one");
            assert_eq!(next, expected);
            views.set(next);
        }
        assert_eq!(views.step(true), None, "NextView at view 5 is a no-op");
        // ...and back down, one at a time, to the bottom.
        for expected in (0..VIEW_COUNT - 1).rev() {
            let prev = views.step(false).expect("a view below this one");
            assert_eq!(prev, expected);
            views.set(prev);
        }
        assert_eq!(views.step(false), None);
    }

    /// `FlipCameraYaw(180)` twice is the identity, mod 2pi — the binding body 1.12 ships, applied
    /// the way [`drain_view_requests`] applies it.
    #[test]
    fn flipping_the_yaw_twice_returns_the_original() {
        let flip = |yaw: f32, degrees: f32| wrap_pi(yaw + degrees.to_radians());
        for start in [0.0_f32, 1.3, -2.9, std::f32::consts::PI] {
            let once = flip(start, 180.0);
            assert!(
                (wrap_pi(once - start).abs() - std::f32::consts::PI).abs() < 1e-5,
                "one flip is half a turn"
            );
            let twice = flip(once, 180.0);
            assert!(
                wrap_pi(twice - start).abs() < 1e-5,
                "two flips return: {twice} vs {start}"
            );
        }
    }

    /// A hand-edited `config.toml` can never land an unreachable view: the distance clamps to the
    /// zoom range, the pitch to +-89 deg, and any yaw encoding (the reference's `[0, 360]`
    /// validator or `SaveView`'s signed offset) wraps into range.
    #[test]
    fn a_hand_edited_view_cannot_land_an_illegal_pose() {
        let wild = ViewPose::from_reference(999.0, 400.0, 720.0 + 90.0);
        assert_eq!(wild.distance, CAM_DIST_MAX);
        assert_eq!(wild.pitch, -CAM_PITCH_LIMIT);
        assert!((wild.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(
            ViewPose::from_reference(-5.0, -400.0, 0.0).distance,
            CAM_DIST_MIN
        );
        assert_eq!(
            ViewPose::from_reference(-5.0, -400.0, 0.0).pitch,
            CAM_PITCH_LIMIT
        );
    }

    /// **The weld** ([`crate::cvars`]'s own convention): every one of the sixteen names is
    /// registered, with the reference's default string. No observer watches them — this module
    /// is their writer, and the registry persists a host write on its own (2303).
    #[test]
    fn the_view_cvars_are_registered_with_the_references_defaults() {
        let registered = |name: &str| {
            crate::cvars::REGISTERED
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(name))
                .map(|r| r.default)
        };
        for view in 0..VIEW_COUNT {
            for field in 0..3 {
                let name = VIEW_CVARS[view][field];
                assert_eq!(
                    registered(name),
                    Some(VIEW_DEFAULTS[view][field]),
                    "{name} is not registered with its shipped default"
                );
            }
        }
        assert_eq!(registered(CVAR_ACTIVE_VIEW), Some(ACTIVE_VIEW_DEFAULT));
    }
}
