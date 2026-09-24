//! **The exterior scene draws only through doorways you can see.**
//!
//! Standing inside a WMO interior, the reference does not draw the outdoor world at large — it draws
//! it once per *portal window* left over by the interior portal flood, with the view frustum narrowed
//! to that window. We drew it unconditionally, which is why a hillside tree 200 yd outside Stratholme
//! showed through the city's walls (the director's report).
//!
//! ## The carved law
//!
//! The world-scene driver `0x681070` branches at `0x681101` on `[0xc7b748]`, the camera-containing map
//! object (0 = outdoors):
//!
//! - **Outside leg** (`0x6811ca`): one populate walk `0x682fa0` against the literal full-screen rect
//!   `{0, 0, 1, 1}` — the ordinary frustum. [`ExteriorWindows::Unrestricted`].
//! - **Inside leg** (`0x681120`): the flood leaves a deferred window worklist (count `[0xcbe320]`,
//!   records `0xcbe324`, stride `0x14`), which the driver `rep movsd`s into `0xc7cb7c` (`0x68118b`)
//!   and then walks **once per window** (`0x682fa0`), frustum narrowed to that window's rect.
//!   **`0x681199 jbe 0x681204` skips the entire walk when the count is zero** — a sealed room draws no
//!   exterior at all.
//!
//! `0x682fa0` is the *only* producer for every exterior bucket — ADT terrain (`0x683bf0`), ADT doodads
//! (`0x683700`), the second placement walk (`0x683340`), world WMO placements (`0x6856c0`), all three
//! liquid layers (`0x683ab0`) and the far band (`0x683040`) — and each drain unlinks what it walks. So
//! "no window" really does mean "no exterior content", with no second path to leak through.
//!
//! ## Bodies ride the same walk — the correction 0774 needed
//!
//! 0774 read this lane one stage too late and wrote the opposite: *"units are exempt and must stay
//! exempt … the reference submits outdoor mobs from a sealed room and lets the building's own
//! geometry z-reject them."* The **drain** fact it checked is true — `0x483460`/`0x48368a` walk the
//! M2Scene worklist with zero references to `[0xc7b748]`/`[0xcbe320]` — but the decision is taken
//! *before* the worklist, in the driver that does read them.
//!
//! Every frame `0x683dd0` (sole caller `0x6812c5`, inside `0x681070`) elects each scene object into
//! one of two passes: `[record+0xb4]` is `0x481540` and nothing else (that dword occurs once
//! image-wide, at `0x613ece`), and `CGObject::ShouldRender` is literally `0x6146c6: and eax,1`.
//! Pass 2 takes `0x48161f je` → `0x48174e call 0x710c50`, which writes `[model+0x50] = 0`: the model
//! never enters `[scene+0x24]`, the only list the draw loop `0x707680`/`0x707882` walks — and walk 2
//! also skips `0x710b90`, so a pass-2 object is neither drawn nor ticked.
//!
//! **List A's only outdoor producer is `0x683340`, whose only caller is `0x682fe2` — inside
//! `0x682fa0`**, the per-window exterior walk above. A sealed room never runs it, so `0x680390`
//! sweeps every outdoor object in the world into pass 2. Nothing is submitted, so there is nothing
//! for the building's geometry to z-reject: 0774's second claim fails in both halves. An object
//! standing in a WMO group rides the sibling `0x6834e0` instead, which is why a creature in the
//! camera's own room still draws.
//!
//! So a body is exterior scene exactly when it is not in a WMO room, and the windows gate it like
//! the ground it stands on — [`WorldUnit::bound`].
//!
//! **The outdoor half of the same election.** The reference's OUTSIDE leg is not a
//! stand-down — it is one full-screen window through the same walk (`0x6811ff`), whose producer
//! `0x683340` frustum-tests every node; and the sibling in-room producer `0x6834e0` submits a
//! group's members only when the group is rendered, frustum-testing each. So outdoors a body's
//! bound is tested against the camera's own view volume, and an in-room body draws only when its
//! room (one portal hop padded — the doorway-straddle guard) is in the PVS *and* its box is in
//! view. 1270 §6 deferred this half; 1473's audit is why it landed. `WOW_NO_DRAW_ELECTION=1` is
//! the lever back to the window leg alone. Knowingly absent: the horizon-occlusion term
//! (`0x686000`) — its own machinery, its own record.
//!
//! ## The clip geometry
//!
//! Per window, the reference bilerps the camera's four global corner rays (`0xc7bcd8`) by the window's
//! rect into 8 corners, and `0x6865f0`→`0x686640` builds **6 planes** from them. So the clip volume is
//! a **sub-frustum whose cross-section is the portal's screen-space AABB** (accumulated along the
//! portal chain), *not* its polygon — and the test is **per object, whole AABB** (`0x682f40`), never
//! per primitive. There is no scissor rect and no user clip plane anywhere on the path: a doodad
//! straddling the doorway edge is drawn **whole**, and the silhouette comes from the interior geometry
//! in front of it.
//!
//! We build the same volume the cheap way: an NDC rect is a scale+offset on clip space, so
//! `sub_clip_from_world = rect_to_ndc * clip_from_world` and Bevy's own
//! [`Frustum::from_clip_from_world`] extracts the identical 6 planes. That keeps us on one
//! plane-extraction implementation instead of a private corner-ray port.
//!
//! Windows narrower than [`MIN_WINDOW_NDC`] in either axis are dropped **on this walk** — the
//! reference's reject is the first thing `0x682fa0` does, before it touches a bucket, and it is
//! non-strict (`>= 0.01` of the 0..1 screen passes). It belongs to the walk, not to the volume:
//! `0x682930`, the builder the containing building's own Pass 2 shares, contains no comparison at
//! all, so a doorway too narrow to admit a hillside can still admit a wall. That asymmetry is the
//! reference's, and reproducing it is [`scene_window_frustum`] vs [`window_frustum`] (decision
//! 1853).
//!
//! ## What is gated so far, and what is knowingly not
//!
//! **Gated:** ADT terrain (`0x683bf0`), ADT doodad placements (`0x683700`), the WDL far band
//! (`0x683040`), the net **bodies** on `0x683340` ([`WorldUnit::bound`]), and open-world **liquid**
//! (`0x683ab0`) — the buckets whose entities have no other `Visibility` writer, so this is their
//! sole authority. World **WMO placements** (`0x6856c0`), their props and their
//! MLIQ pools are gated too, but by the *model-visibility* authority, which folds [`ExteriorGate`]
//! into its own AND rather than being written here.
//!
//! **Liquid was the last of them**. 0774 deferred it and 0784 deferred it again,
//! both on the same stated ground — that it "already has a visibility authority… its own lane". It
//! did not. An ADT surface is spawned as a world root with `Mesh3d`, and nothing ever wrote its
//! `Visibility`; the only writer in the whole subsystem was the `$WOW_NO_LIQUID` kill-switch,
//! env-gated and `Added`-filtered. So the fix was not the "real design step" both records
//! predicted — it was the tag, and this system was already the right owner. The lesson is 0784's
//! own, one turn later: *"it already has an authority" needs a check, not a memory.* 0774 made that
//! claim about two buckets at once; it was true of WMO placements and false of liquid, and because
//! the two travelled in one sentence, nobody separated them. 0784 checked the half it was closing,
//! found the claim inverted there, and copied the other half forward untested. What that cost is
//! the director's report — from inside a cavern, the lake overhead drawing straight through the
//! ceiling.
//!
//! **The unit is the drawn object, and for terrain that is the 33.333 yd MCNK cell, not the 533 yd
//! tile**. This is a property of the *spawner*, not of anything here — but the cull is
//! where it bites: a tile-sized box always contains the camera's own ground, so it intersects every
//! window and is admitted whichever way the doorway faces. The cull was correct and looked broken.
//! The far band keeps its whole-tile box, which is the reference's own far-tier granularity.
//!
//! **NOT gated, knowingly:** the horizon-occlusion term (`0x686000`) — its own machinery, its
//! own record. Nothing in the exterior *buckets* is left ungated.
//!
//! **Deviation to be honest about:** the reference tests one AABB per *object*; a doodad placement has
//! no root entity in our graph, so we tag and test each **submesh**. Their union is the object, so the
//! only divergence is at a doorway's edge — a submesh entirely outside the window is dropped where the
//! reference would draw the doodad whole. Visible only as a partially-clipped prop in a doorway.
//! A body does not take that deviation: it has a root, so it is tested once, whole ([`ElectedBody`]).

use bevy::camera::primitives::{Aabb, Frustum};
use bevy::camera::visibility::VisibilitySystems;
use bevy::prelude::*;

use crate::view::WorldCamera;
use crate::wmo_portal::{ExteriorWindows, Rect, WmoPvsSet};

/// A window narrower than this fraction of the screen in either axis is dropped — the reference's
/// `0x682fa0` test against `[0x8029d0]` (0.01 of a 0..1 screen; our rects are NDC, so twice that).
const MIN_WINDOW_NDC: f32 = 0.02;

/// Tag for a piece of the **exterior scene** — the content the window worklist gates. Put this on ADT
/// terrain **chunks**, ADT doodad placements, the WDL far band, world WMO placements and open-world
/// liquid. Tag the object that is *drawn*: this is tested per entity, so tagging a container whose box
/// spans far more than any one draw admits the lot.
///
/// **Never on anything parented to a WMO group** — those are already culled by the portal PVS
/// ([`crate::wmo_portal::WmoGroupVis`]), and double-gating them would blank building interiors.
/// **Never on a net body either**, but for the opposite reason to the one this line used to give:
/// a body IS elected (see the module header), it is simply elected once at its **root** off
/// [`WorldUnit::bound`], rather than per submesh.
#[derive(Component)]
pub struct ExteriorScene;

/// Ordering handle: [`ExteriorWindows`] is consumed and `Visibility` written after this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExteriorCullSet;

/// What the cull actually did on its last run — the instrument that makes a "why is that still
/// drawn?" report answerable without guessing. `windows` is the worklist it read (`None` =
/// [`ExteriorWindows::Unrestricted`], the stand-down leg), `frusta` how many survived
/// [`MIN_WINDOW_NDC`], and `tested`/`hidden` how many tagged objects it reached and rejected.
///
/// Its whole point is that `tested` is the number no other instrument can see: an object drawn
/// through a wall while `hidden == tested` is an object the cull **never reached**, which is a
/// different defect from one it reached and admitted.
#[derive(Resource, Default, Clone, Copy)]
pub struct ExteriorCullVerdict {
    pub(crate) windows: Option<usize>,
    pub(crate) frusta: usize,
    pub(crate) tested: usize,
    pub(crate) hidden: usize,
    /// Tagged objects with **no `Aabb`**, which the fail-open arm below admits unconditionally.
    /// Non-zero is not automatically a defect (a tagged non-drawing entity has no bound and draws
    /// nothing), but it is the one number that says how much of the scene the cull is not deciding.
    pub(crate) unbounded: usize,
    /// Net bodies the election reached ([`WorldUnit::bound`] set), and how many the windows
    /// rejected — counted apart from
    /// `tested`/`hidden` because they answer a different question. The world buckets are a
    /// *residency* count in the tens of thousands; bodies are the server's visibility stream, a
    /// couple of dozen. Summed together, a body leg that reaches nothing at all would be invisible.
    pub(crate) bodies: usize,
    pub(crate) bodies_hidden: usize,
    /// Open-world liquid surfaces the cull reached, and how many the windows rejected — a
    /// **subset** of `tested`/`hidden` above, reported separately for the reason the body leg is.
    /// A tile carries a handful of MCNK liquid layers against ~250 terrain cells,
    /// so a liquid leg that reached *nothing* would move `tested` by less than its own noise. This
    /// is the number that says "the lake is being decided", and before 1652 it was structurally
    /// zero.
    pub(crate) liquid: usize,
    pub(crate) liquid_hidden: usize,
}

pub(crate) struct ExteriorCullPlugin;

impl Plugin for ExteriorCullPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("WOW_CULL_TRACE").is_some() {
            app.insert_resource(CullTrace);
        }
        app.init_resource::<ExteriorCullVerdict>().add_systems(
            PostUpdate,
            apply_exterior_cull
                .in_set(ExteriorCullSet)
                // The windows are written by the flood in `Update`; the cull reads them and must land
                // before Bevy's own visibility pass consumes the result this same frame — and before
                // the hierarchy propagation, so a root verdict reaches `InheritedVisibility` (which
                // 1283's consumers and the billboard mirror read) the SAME frame. 1270 only ordered
                // against `CheckVisibility`, leaving a benign race the window leg never noticed; the
                // frustum leg follows the camera every frame, so the frame of lag is pinned away.
                .after(WmoPvsSet)
                .after(bevy::transform::TransformSystems::Propagate)
                .before(VisibilitySystems::VisibilityPropagate)
                .before(VisibilitySystems::CheckVisibility),
        );
    }
}

/// The sub-frustum for one NDC window rect — the **construction alone**, no narrowness gate.
///
/// **Two callers, deliberately one construction**: this module gates the open world
/// through these rects, and `crate::wmo_portal`'s Pass 2 gates the containing building's own
/// exterior groups through the same ones. The reference likewise builds one volume per window and
/// hands it to both (`0x682930` for `0x6b3b20`'s Pass 2, and the copy at `0xc7cb7c` for
/// `0x682fa0`). What 1826 also copied across, and should not have, was the *reject*: it belongs to
/// the open-world walk's entry, not to the volume — see [`scene_window_frustum`]. So a doorway too
/// narrow to admit a hillside **can** still admit a wall, which is the reference's own asymmetry.
///
/// An NDC rect maps to the full screen by a scale+offset **on clip space** — `clip.x` scaled by
/// `2/(x1-x0)` plus `clip.w` times `-(x0+x1)/(x1-x0)`, and likewise in y — so pre-multiplying
/// `clip_from_world` by that matrix yields a projection whose 6 extracted planes bound exactly the
/// window's sub-frustum. Depth rows are untouched: the window narrows the view laterally and inherits
/// the camera's own near/far.
pub(crate) fn window_frustum(rect: Rect, clip_from_world: &Mat4) -> Frustum {
    let [x0, y0, x1, y1] = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    // Column-major: `Mat4::from_cols` takes columns, so the `w`-column carries the offsets.
    let rect_to_ndc = Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(-(x0 + x1) / w, -(y0 + y1) / h, 0.0, 1.0),
    );
    Frustum::from_clip_from_world(&(rect_to_ndc * *clip_from_world))
}

/// [`window_frustum`] behind the **open-world walk's own entry gate** — and nothing else's.
///
/// `0x682fa0` opens with the gate, before it touches a bucket: `fld [edi+8]; fsub [edi]; fcomp
/// [0x8029d0]; test ah,5; jnp -> ret` on x, then the same on y. `[0x8029d0]` is `0.01` and the rects
/// are `[0,1]` screen fraction, so the walk runs iff **both extents are `>= 0.01`** — 0.02 in our NDC,
/// non-strict, and a window under it draws no terrain, no ADT doodads, no world WMO placement, no
/// liquid, no far band and no outdoor unit at all.
///
/// **The gate is the walk's, not the volume's**: a compare-class census over the whole of
/// `0x682930` — the builder both consumers share — returns **zero** compares, and Pass 2's own body
/// `[0x6b3c73, 0x6b3d6f)` has seven, none of them on the rect. Decision 1826 read the reject as
/// part of the shared construction and applied it to both, which silently culled the containing
/// building's whole exterior shell for every window between the recursion's own collapse guard
/// (`0.001` NDC, `0x6b44b1`) and this one.
pub(crate) fn scene_window_frustum(rect: Rect, clip_from_world: &Mat4) -> Option<Frustum> {
    let [x0, y0, x1, y1] = rect;
    if x1 - x0 < MIN_WINDOW_NDC || y1 - y0 < MIN_WINDOW_NDC {
        return None;
    }
    Some(window_frustum(rect, clip_from_world))
}

/// This frame's exterior gate — the window worklist turned into clip volumes, built once and asked
/// per object. **Two systems ask it**, which is the whole point of it being a value rather than a
/// loop: [`apply_exterior_cull`] owns the exterior objects nothing else writes (terrain cells, the
/// far band), and [`crate::debug_panel`]'s model-visibility authority folds the same answer into its
/// own AND for every model submesh, because those already have an owner.
pub enum ExteriorGate {
    /// Outdoors — the driver's outside leg (`0x6811ca`): the ordinary frustum is the window, so the
    /// gate admits everything and each object goes back to whatever else owns it.
    Open,
    /// Indoors: an object draws only where one of these sub-frusta admits it. **Empty admits
    /// nothing** — the sealed-room case (`0x681199`'s skip), and the one branch that must not fail
    /// open, because failing open here is the bug this whole module exists to fix.
    Windows(Vec<Frustum>),
}

impl ExteriorGate {
    /// Build from the worklist + the world camera. No camera yet ⇒ [`Self::Open`]: nothing is on
    /// screen to leak, and a verdict taken without a view matrix would be arbitrary.
    pub(crate) fn build(
        windows: &ExteriorWindows,
        cam: Option<(&GlobalTransform, &Projection)>,
    ) -> Self {
        let ExteriorWindows::Windows(rects) = windows else {
            return Self::Open;
        };
        let Some((cam_t, proj)) = cam else {
            return Self::Open;
        };
        let clip_from_world = proj.get_clip_from_view() * cam_t.to_matrix().inverse();
        Self::Windows(
            rects
                .iter()
                .filter_map(|r| scene_window_frustum(*r, &clip_from_world))
                .collect(),
        )
    }

    /// Whole-AABB, per object — the reference's `0x682f40`. Not per primitive: a doodad straddling
    /// the doorway edge draws whole, and the interior geometry cuts its silhouette.
    ///
    /// No `Aabb` (mesh still loading, or an entity that draws nothing) ⇒ admitted. A missing bound
    /// is a *timing* gap, not a visibility verdict, and blanking on it would flicker the world as
    /// tiles stream in.
    pub(crate) fn admits(&self, gt: &GlobalTransform, aabb: Option<&Aabb>) -> bool {
        match (self, aabb) {
            (Self::Open, _) | (_, None) => true,
            (Self::Windows(frusta), Some(aabb)) => {
                let world_from_local = gt.affine();
                frusta
                    .iter()
                    .any(|f| f.intersects_obb(aabb, &world_from_local, true, true))
            }
        }
    }

    /// The same test against a WORLD-space bounding **sphere** — the form the particle lane speaks
    /// (`EmitterFade`'s `[rec+0x68]` fade sphere is the owner doodad's bound, and an emitter has no
    /// mesh of its own to carry an `Aabb`). The sphere's axis-aligned box is what gets tested, which
    /// is looser than the sphere and therefore never hides something a sphere test would admit.
    pub(crate) fn admits_sphere(&self, center: Vec3, radius: f32) -> bool {
        let aabb = Aabb::from_min_max(center - Vec3::splat(radius), center + Vec3::splat(radius));
        self.admits(&GlobalTransform::IDENTITY, Some(&aabb))
    }

    fn frusta(&self) -> usize {
        match self {
            Self::Open => 0,
            Self::Windows(f) => f.len(),
        }
    }
}

/// What [`apply_exterior_cull`] reads per object.
type UnownedScene = (
    &'static GlobalTransform,
    Option<&'static Aabb>,
    &'static mut Visibility,
    // Instrument only — never a term in the verdict. Liquid takes exactly the same test as a
    // terrain cell; this only lets the counters say so (see [`ExteriorCullVerdict::liquid`]).
    Has<crate::liquid::LiquidSurface>,
);

/// …and the objects it is allowed to write: exterior scene that nothing else owns. `ModelPart` and
/// `WmoGroupVis` both mean "the model-visibility authority writes this one".
type UnownedSceneFilter = (
    With<ExteriorScene>,
    Without<crate::model_render::ModelPart>,
    Without<crate::wmo_portal::WmoGroupVis>,
    // …and disjoint from the body leg below, which also writes `Visibility`. The two audiences
    // never overlap by construction (a body is elected at its root, which is not scene content),
    // but Bevy needs that **proved**, not intended: without this the system panics B0001 at the
    // first run. Stating it here rather than in the body query keeps the exclusion next to the
    // `ModelPart`/`WmoGroupVis` ones, which are the same kind of claim.
    Without<crate::world_unit::WorldUnit>,
);

/// **`WOW_CULL_TRACE=1` — one line per body per frame, saying why it drew.**
///
/// `cull_bodies`/`cull_bodies_hidden` say *how many*; they cannot say *which*, and the difference
/// cost three live runs of inference during. A body escapes this cull in exactly three
/// ways — it was never elected (no `WorldUnit::bound`), it claims a WMO room, or a window admitted
/// its box — and from a screenshot, and from the aggregate counters, all three look identical.
///
/// Off (the var unset) this is one `Option` read per body. Deliberately not throttled like
/// `WOW_VIS_TRACE`: the audience is the server's visibility stream (dozens), and the question it
/// answers — "which body, on which frame" — is destroyed by sampling.
fn trace_body(
    trace: &Option<Res<CullTrace>>,
    e: Entity,
    class: &str,
    gt: &GlobalTransform,
    room: Option<&crate::wmo_portal::UnitWmoRoom>,
    bound: Option<&Aabb>,
    admitted: bool,
) {
    if trace.is_none() {
        return;
    }
    let p = gt.translation();
    println!(
        "CULL_BODY {e} {class} admitted={admitted} claim={claim} bound={bound} \
         pos=[{:.1},{:.1},{:.1}]",
        p.x,
        p.y,
        p.z,
        claim = match room.map(|r| r.room()) {
            None => "absent".to_string(),
            Some(None) => "outdoors".to_string(),
            Some(Some(r)) => format!("g{}", r.group),
        },
        bound = match bound {
            None => "none".to_string(),
            Some(b) => format!("r{:.1}", Vec3::from(b.half_extents).length()),
        },
    );
}

/// `WOW_CULL_TRACE` — see [`trace_body`].
#[derive(Resource)]
pub(crate) struct CullTrace;

/// What the body leg reads per net body — one whole-object test on its root, which is the
/// reference's own granularity (`0x682f40`), and which takes the whole visual with it: body
/// geosets, worn gear under their joints, the mount and its rider (`mount::spawn_mount_child`
/// parents the mount to its host), and the entity-lane billboard cards, which are world roots that
/// mirror their owner's `InheritedVisibility` ([`crate::billboard::face_billboards`]). None of that
/// composes from a per-submesh tag.
///
/// [`crate::wmo_portal::UnitWmoRoom`] is the reference's own fork between its two producers: a room
/// claim means the object rides `0x6834e0` with its building, no claim means it rides `0x683340`
/// inside the window walk. `Option` because a body that spawned this frame may not have been rayed
/// yet, and an unknown room is a **timing gap, not a verdict** — the same law the missing-`Aabb`
/// arm above takes, and for the same reason: deciding on absent information flickers the world.
type ElectedBody = (
    Entity,
    &'static GlobalTransform,
    &'static crate::world_unit::WorldUnit,
    Option<&'static crate::wmo_portal::UnitWmoRoom>,
    &'static mut Visibility,
);

/// Hide every [`ExteriorScene`] object that no window admits — **only those with no other
/// `Visibility` owner**. A model submesh (`ModelPart`) and a WMO group piece (`WmoGroupVis`) are
/// written by [`crate::debug_panel`]'s model-visibility authority, which composes the toggles, the
/// far-clip wall, the distance fade and the portal PVS; a second writer here does not "also cull"
/// them, it *overwrites* all of that every frame. So this system owns exactly what
/// it is the sole authority for: terrain cells, the WDL far band, and — in the second query below,
/// a different audience under the same law rather than a second system — the elected net bodies
/// ([`ElectedBody`]), whose roots nothing else writes.
// A Bevy system's params are not an argument list to shorten — each is a distinct world access
// the scheduler needs by name (the `update_ground_shade` precedent).
fn apply_exterior_cull(
    windows: Res<ExteriorWindows>,
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    mut scene: Query<UnownedScene, UnownedSceneFilter>,
    mut bodies: Query<ElectedBody>,
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
    wmos: Res<Assets<benilla_assets::WmoModel>>,
    mut verdict: ResMut<ExteriorCullVerdict>,
    trace: Option<Res<CullTrace>>,
    mut no_election: Local<Option<bool>>,
) {
    // `WOW_NO_DRAW_ELECTION=1` — disable 1475's frustum + room legs only (the 1270 window leg
    // stays: it fixes a *correctness* report, not a cost). The A/B lever that brackets the
    // election's worth at any pin, and the escape hatch if an artifact is ever reported.
    let no_election =
        *no_election.get_or_insert_with(|| std::env::var_os("WOW_NO_DRAW_ELECTION").is_some());
    let set = |vis: &mut Visibility, target: Visibility| {
        if *vis != target {
            *vis = target;
        }
    };
    let gate = ExteriorGate::build(&windows, cam.iter().next());
    // The camera's own view volume, as the full-screen window: the reference's OUTSIDE leg is
    // literally one `{0,0,1,1}` window through the same walk (`0x6811ff`), so outdoors is not a
    // stand-down for bodies — an out-of-frustum body is elected pass 2, not drawn and not ticked
    // (1473's correction of 0648). Built from this frame's own camera transform
    // (we run after `Propagate`), never the `Frustum` component, which another PostUpdate set
    // may not have refreshed yet. `None` (no camera) admits: a verdict without a view matrix
    // would be arbitrary — `ExteriorGate::build`'s own law.
    let full_view = cam.iter().next().and_then(|(cam_t, proj)| {
        let clip_from_world = proj.get_clip_from_view() * cam_t.to_matrix().inverse();
        scene_window_frustum([-1.0, -1.0, 1.0, 1.0], &clip_from_world)
    });
    let in_view = |gt: &GlobalTransform, bound: &Aabb| {
        full_view
            .as_ref()
            .is_none_or(|f| f.intersects_obb(bound, &gt.affine(), true, true))
    };
    let (mut tested, mut hidden, mut unbounded) = (0usize, 0usize, 0usize);
    let (mut liquid, mut liquid_hidden) = (0usize, 0usize);
    for (gt, aabb, mut vis, is_liquid) in &mut scene {
        tested += 1;
        unbounded += usize::from(aabb.is_none());
        let admitted = gate.admits(gt, aabb);
        hidden += usize::from(!admitted);
        liquid += usize::from(is_liquid);
        liquid_hidden += usize::from(is_liquid && !admitted);
        set(
            &mut vis,
            if admitted {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        );
    }
    // The body leg: one whole-object test per net body (see [`ElectedBody`]), serial because the
    // audience is the server's visibility stream — dozens, not the tens of thousands above.
    let (mut body_n, mut body_hidden) = (0usize, 0usize);
    for (e, gt, body, room, mut vis) in &mut bodies {
        let Some(bound) = body.bound.as_ref() else {
            // The game says this body is not ours to decide (`WorldUnit::bound`). Traced too: an
            // un-elected body is invisible to every counter below, which is exactly how a whole
            // class of creature sat outside this cull unnoticed.
            trace_body(&trace, e, "unelected", gt, room, None, true);
            continue;
        };
        body_n += 1;
        // In a WMO room ⇒ its building's own walk submits it, not the exterior one.
        //
        // **No claim yet reads as outdoors, not as exempt**, and that asymmetry is the whole of the
        // director's second report. A claim is `try_insert`ed by `track_unit_interiors` a frame or
        // two after a body spawns, so "undecided ⇒ admit" let every streaming mob draw through a
        // sealed ceiling for exactly as long as the gap — which at a cavern's frame rate is most of
        // a second, per mob, continuously. The two error directions are not symmetric: guessing
        // *outdoors* makes a mob in your own room appear a frame late, guessing *in a room* makes a
        // mob outside appear through solid rock. And `UnitWmoRoom::default()` is itself a
        // no-room claim, so this simply decides the missing component the way the present one would.
        let in_room = room.is_some_and(|r| r.room().is_some());
        let admitted = if no_election {
            // The lever: 1270's window leg alone — in-room exempt, outdoors a stand-down.
            in_room || gate.admits(gt, Some(bound))
        } else if in_room {
            // The sibling producer (`0x6834e0`): submitted with its building iff its room — or a
            // one-hop neighbour, the doorway-straddle guard — is in this frame's portal PVS,
            // AND its box passes the camera frustum like any other group member (the producer
            // frustum-tests members; a body behind the camera in a visible room is pass 2).
            // Fail-open at every resolve seam ([`crate::wmo_portal::room_pvs_visible`]).
            crate::wmo_portal::room_pvs_visible(room, &instances, &wmos) && in_view(gt, bound)
        } else {
            // The exterior walk: outdoors, the one full-screen window; indoors, the portal
            // windows — which are already sub-frusta of the view, so no second frustum term.
            match &gate {
                ExteriorGate::Open => in_view(gt, bound),
                ExteriorGate::Windows(_) => gate.admits(gt, Some(bound)),
            }
        };
        trace_body(
            &trace,
            e,
            if in_room { "in-room" } else { "outdoor" },
            gt,
            room,
            Some(bound),
            admitted,
        );
        body_hidden += usize::from(!admitted);
        set(
            &mut vis,
            if admitted {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        );
    }
    *verdict = ExteriorCullVerdict {
        windows: match &*windows {
            ExteriorWindows::Unrestricted => None,
            ExteriorWindows::Windows(rects) => Some(rects.len()),
        },
        frusta: gate.frusta(),
        tested,
        hidden,
        unbounded,
        bodies: body_n,
        bodies_hidden: body_hidden,
        liquid,
        liquid_hidden,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Affine3A;

    /// A camera at the origin looking down −Z (Bevy's convention), 90° fov, square aspect.
    fn clip_from_world() -> Mat4 {
        Projection::Perspective(PerspectiveProjection {
            fov: std::f32::consts::FRAC_PI_2,
            aspect_ratio: 1.0,
            near: 0.1,
            far: 1000.0,
            ..default()
        })
        .get_clip_from_view()
    }

    /// A point admitted by a frustum? (A degenerate AABB at `p`, identity placement.)
    fn admits(f: &Frustum, p: Vec3) -> bool {
        let aabb = Aabb::from_min_max(p - Vec3::splat(0.01), p + Vec3::splat(0.01));
        f.intersects_obb(&aabb, &Affine3A::IDENTITY, true, true)
    }

    /// The full-screen window must behave exactly like the ordinary view frustum — this is the
    /// outside leg's `{0,0,1,1}` rect, and if it ever narrowed, standing outdoors would start
    /// clipping the world.
    #[test]
    fn the_full_screen_window_is_the_whole_frustum() {
        let f = window_frustum([-1.0, -1.0, 1.0, 1.0], &clip_from_world());
        assert!(admits(&f, Vec3::new(0.0, 0.0, -10.0)), "straight ahead");
        // At 90° fov the frustum edge is |x| = |z|; well inside it must pass, and behind must not.
        assert!(admits(&f, Vec3::new(5.0, 0.0, -10.0)), "right of centre");
        assert!(admits(&f, Vec3::new(-5.0, 0.0, -10.0)), "left of centre");
        assert!(!admits(&f, Vec3::new(0.0, 0.0, 10.0)), "behind the eye");
    }

    /// The load-bearing property: a window covering only the RIGHT half of the screen must admit
    /// what is on the right and reject what is on the left. This is the whole cull — a doorway on
    /// one side of the view must not let the world in on the other.
    #[test]
    fn a_half_screen_window_rejects_the_other_half() {
        let f = window_frustum([0.1, -1.0, 1.0, 1.0], &clip_from_world());
        assert!(admits(&f, Vec3::new(5.0, 0.0, -10.0)), "right: admitted");
        assert!(
            !admits(&f, Vec3::new(-5.0, 0.0, -10.0)),
            "left: must be rejected — the window is on the right"
        );
    }

    /// **The granularity of the tagged object is half the cull**. This test is the
    /// same patch of ground twice: once as the 33.333 yd MCNK cell it is drawn as now, once as the
    /// 533.333 yd ADT tile it used to be merged into. The cell is rejected; the tile is admitted,
    /// because a tile is drawn around the camera and so reaches the doorway's side of the view no
    /// matter which side the ground is on. Nothing in `apply_exterior_cull` can fix that — it is
    /// decided by what the spawner tags — which is why re-merging terrain into a per-tile mesh
    /// would silently restore "I can see the hillside through the wall" with every test still green.
    #[test]
    fn a_narrow_window_rejects_a_chunk_of_ground_but_never_a_whole_tile_of_it() {
        // A doorway on the RIGHT of the view; the ground of interest is 200 yd to the LEFT.
        let f = window_frustum([0.6, -1.0, 1.0, 1.0], &clip_from_world());
        let ground = Vec3::new(-200.0, -2.0, -200.0);

        const CELL: f32 = 33.333 / 2.0;
        let cell = Aabb::from_min_max(
            ground - Vec3::new(CELL, 0.5, CELL),
            ground + Vec3::new(CELL, 0.5, CELL),
        );
        assert!(
            !f.intersects_obb(&cell, &Affine3A::IDENTITY, true, true),
            "an MCNK cell to the left must not come in through a doorway on the right"
        );

        // The ADT tile that CONTAINS that cell. The camera stands on it, so it spans both sides of
        // the view — half a kilometre of ground admitted by a doorway that shows a hundredth of it.
        const TILE: f32 = 533.333 / 2.0;
        let tile = Aabb::from_min_max(Vec3::new(-TILE, -2.5, -TILE), Vec3::new(TILE, -1.5, TILE));
        assert!(
            f.intersects_obb(&tile, &Affine3A::IDENTITY, true, true),
            "a tile-sized box reaches the doorway's side of the view — it can never be culled, and \
             that is exactly why terrain is spawned per chunk"
        );
    }

    /// The OPEN-WORLD walk rejects a window narrower than a hundredth of the screen
    /// (`0x682fa0`'s entry compares against `[0x8029d0]`) — and it is non-strict, so a window
    /// exactly at the threshold draws. The reject also keeps a collapsed rect from producing a
    /// degenerate frustum whose planes are NaN (which `intersects_obb` would answer arbitrarily).
    #[test]
    fn the_scene_walk_drops_a_window_under_a_hundredth_of_the_screen() {
        assert!(scene_window_frustum([0.5, -1.0, 0.5, 1.0], &clip_from_world()).is_none());
        assert!(scene_window_frustum([-1.0, 0.2, 1.0, 0.2001], &clip_from_world()).is_none());
        // `>= 0.01` of a 0..1 screen passes — the client's `jnp` is parity, not a strict `>`.
        assert!(
            scene_window_frustum([0.0, -1.0, -1.0 + MIN_WINDOW_NDC, 1.0], &clip_from_world())
                .is_none(),
            "a rect with a NEGATIVE extent is nonsense and must not build a frustum"
        );
        assert!(
            scene_window_frustum([0.0, -1.0, MIN_WINDOW_NDC, 1.0], &clip_from_world()).is_some()
        );
    }

    /// …and the volume builder itself has no such gate, because `0x682930` has no compares at all:
    /// Pass 2 draws the containing building's own shell through a window the open world cannot use.
    /// The band is `[0.001, 0.02)` NDC — above the recursion's collapse guard,
    /// below the scene walk's entry.
    #[test]
    fn pass_twos_builder_takes_a_window_the_scene_walk_rejects() {
        let narrow = [0.0, -1.0, 0.01, 1.0];
        assert!(scene_window_frustum(narrow, &clip_from_world()).is_none());
        let f = window_frustum(narrow, &clip_from_world());
        // fov 90 / aspect 1, so NDC x = world x / -z: the sliver's axis at z = -10 is x = 0.05.
        assert!(
            admits(&f, Vec3::new(0.05, 0.0, -10.0)),
            "the sliver still bounds a real volume: a point straight down its axis is inside"
        );
        assert!(
            !admits(&f, Vec3::new(5.0, 0.0, -10.0)),
            "…and it is still a sliver: NDC x = 0.5 is far outside it"
        );
    }

    /// One `apply_exterior_cull` run over a scene built by the caller, returning each body's
    /// verdict in spawn order plus the counters.
    fn run_bodies(
        windows: ExteriorWindows,
        bodies: &[(Vec3, Option<crate::wmo_portal::UnitWmoRoom>, Aabb)],
    ) -> (Vec<Visibility>, ExteriorCullVerdict) {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            .insert_resource(windows)
            .add_systems(Update, apply_exterior_cull);
        // The same camera the pure-frustum tests use: origin, down −Z, 90° fov.
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        let ids: Vec<Entity> = bodies
            .iter()
            .map(|(at, room, bound)| {
                let mut e = app.world_mut().spawn((
                    GlobalTransform::from_translation(*at),
                    crate::world_unit::WorldUnit {
                        wades: true,
                        scale: 1.0,
                        height: 2.0,
                        bound: Some(*bound),
                    },
                    Visibility::Inherited,
                ));
                if let Some(room) = *room {
                    e.insert(room);
                }
                e.id()
            })
            .collect();
        app.update();
        let vis = ids
            .iter()
            .map(|e| *app.world().entity(*e).get::<Visibility>().unwrap())
            .collect();
        (vis, *app.world().resource::<ExteriorCullVerdict>())
    }

    /// The three states a body's room claim can be in — kept distinct because the middle and the
    /// last decide opposite ways and are one `Option` layer apart.
    fn outdoors() -> Option<crate::wmo_portal::UnitWmoRoom> {
        Some(crate::wmo_portal::UnitWmoRoom::default()) // rayed, hit nothing
    }
    fn in_a_room() -> Option<crate::wmo_portal::UnitWmoRoom> {
        // The room's *identity* is nothing to this system — only whether there is one.
        Some(crate::wmo_portal::UnitWmoRoom::claimed(
            crate::wmo_portal::WmoRoom {
                instance: Entity::from_raw_u32(1).expect("a valid entity id"),
                group: 0,
            },
        ))
    }
    const NOT_RAYED_YET: Option<crate::wmo_portal::UnitWmoRoom> = None;

    /// A yard-ish box on the body's own origin — a creature's authored idle CAaBox.
    fn creature_box() -> Aabb {
        Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5))
    }
    /// …and what the game writes before a body's model has resolved: a point at its origin.
    fn unresolved() -> Aabb {
        Aabb::from_min_max(Vec3::ZERO, Vec3::ZERO)
    }

    /// **The Caverns of Time report, as a test**. Sealed room ⇒ no windows ⇒ the
    /// exterior walk never runs, so an outdoor body is not submitted at all — while a body standing
    /// in a WMO room rides the sibling producer `0x6834e0` and still draws. Both bodies sit at the
    /// same spot dead ahead of the camera: the *only* thing separating them is the room claim,
    /// which is the reference's own fork between its two producers.
    #[test]
    fn a_sealed_room_hides_the_outdoor_body_and_keeps_the_one_in_a_room() {
        let ahead = Vec3::new(0.0, 0.0, -50.0);
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(Vec::new()),
            &[
                (ahead, outdoors(), creature_box()),
                (ahead, in_a_room(), creature_box()),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Hidden, Visibility::Inherited],
            "outdoors is elected to pass 2; a body in a room is submitted with its building"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (2, 1));
    }

    /// **A body whose model has not resolved yet is elected all the same**, on the point at its own
    /// origin — the director's second report, after the first cut had landed: creatures still
    /// appearing overhead inside Caverns of Time.
    ///
    /// Every streamed mob drew for one whole frame, because the model's extent reaches
    /// `publish_world_units` only *after* `attach_entity_visuals` has already spawned the visual,
    /// and a bound-less body was admitted. That looked like a conservative default and was a defect:
    /// at a cavern's frame rate one frame is most of a second, and mobs stream continuously, so a
    /// per-body flash reads as "they are still there". The origin never needed waiting for — it is
    /// the server's own position, exact from the body's first frame.
    #[test]
    fn a_body_whose_model_has_not_resolved_is_elected_on_its_origin() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(Vec::new()),
            &[(Vec3::new(0.0, 0.0, -50.0), outdoors(), unresolved())],
        );
        assert_eq!(
            vis,
            vec![Visibility::Hidden],
            "a point at the origin is a testable bound — the sealed room must reject it"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (1, 1));
    }

    /// A body whose room has not been rayed yet reads as **outdoors**, and is gated.
    ///
    /// This test asserted the opposite when the first cut landed — "an unknown room is a timing gap,
    /// not a verdict", borrowed from the missing-`Aabb` arm beside it. The director refuted it from
    /// the screen: creatures still overhead inside Caverns of Time. `track_unit_interiors`
    /// `try_insert`s a claim a frame or two after a body spawns, so on a continuous stream of mobs
    /// there is always a handful mid-gap, and admitting them drew each one through the ceiling for
    /// most of a second.
    ///
    /// The lesson is that the two arms are not the same question. A missing bound leaves nothing to
    /// test, so the cull genuinely cannot decide. A missing *claim* still has a default the
    /// component itself states — `UnitWmoRoom::default()` is a no-room claim — and the error
    /// directions are lopsided: a body wrongly called outdoors appears a frame late, a body wrongly
    /// called indoors appears through solid rock.
    #[test]
    fn an_unrayed_body_reads_as_outdoors() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(Vec::new()),
            &[(Vec3::new(0.0, 0.0, -50.0), NOT_RAYED_YET, creature_box())],
        );
        assert_eq!(
            vis,
            vec![Visibility::Hidden],
            "no claim is not a licence to draw through a sealed room"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (1, 1));
    }

    /// Outdoors the driver's `{0,0,1,1}` leg IS the ordinary frustum — the reference's outside
    /// leg is one full-screen window through the same frustum-testing walk, so a body behind the
    /// camera is elected pass 2: not drawn. This test asserted the opposite under
    /// 1270 ("`Unrestricted` is a stand-down… the arm that must never narrow") — that law was
    /// built before the outside-leg election was corrected and is deliberately superseded here
    /// (1473).
    #[test]
    fn outdoors_the_full_screen_window_elects_the_bodies() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Unrestricted,
            &[
                (Vec3::new(0.0, 0.0, -50.0), outdoors(), creature_box()),
                // Behind the camera: pass 2. Its parts carry `NoFrustumCulling` (0648's picker
                // law), so this root verdict is the ONLY thing standing between an off-view
                // crowd and the draw queue — the Goldshire premium of 1473.
                (Vec3::new(0.0, 0.0, 50.0), outdoors(), creature_box()),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Inherited, Visibility::Hidden],
            "ahead is drawn; behind the camera is elected out"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (2, 1));
    }

    /// The in-room fork of the same election: a body in a PVS-dark room is not drawn — the
    /// sibling producer only submits members of rendered groups — and the moment the flood
    /// reaches its room (or a one-hop neighbour), it draws again. The imp-upstairs-in-the-inn
    /// case from 1473's audit: visible-room wiring, resolve chain and all, not just the truth
    /// table (which lives with [`crate::wmo_portal::room_pvs_visible`]).
    #[test]
    fn a_body_in_a_pvs_dark_room_is_not_drawn_until_the_flood_reaches_it() {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            .insert_resource(ExteriorWindows::Unrestricted)
            .add_systems(Update, apply_exterior_cull);
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        // A two-group building whose groups share no portal: group 0's verdict is its own bit.
        // The nav table must be REAL — an absent nav entry is a fail-open seam that reads
        // visible ([`crate::wmo_portal::room_pvs_visible`]'s law), which is right for a
        // still-loading model and wrong as a test of the dark-room verdict.
        let sealed = |_g: u16| benilla_assets::WmoGroupNav {
            flags: 0,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            ref_start: 0,
            ref_count: 0,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        };
        let model = benilla_assets::WmoModel {
            wmo_id: 1,
            group_nav: vec![sealed(0), sealed(1)],
            portal_refs: Vec::new(),
            ..Default::default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<benilla_assets::WmoModel>>()
            .add(model);
        let inst = app
            .world_mut()
            .spawn(crate::wmo_portal::WmoPortalInstance {
                handle,
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![false, false],
                interior_fog: vec![false, false],
                liquid_visited: vec![false, false],
                flooded: vec![None, None],
            })
            .id();
        // Dead ahead of the camera — only the room term can hide it.
        let body = app
            .world_mut()
            .spawn((
                GlobalTransform::from_translation(Vec3::new(0.0, 0.0, -50.0)),
                crate::world_unit::WorldUnit {
                    wades: true,
                    scale: 1.0,
                    height: 2.0,
                    bound: Some(creature_box()),
                },
                crate::wmo_portal::UnitWmoRoom::claimed(crate::wmo_portal::WmoRoom {
                    instance: inst,
                    group: 0,
                }),
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(body).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "a PVS-dark room's body is not submitted"
        );
        // The flood reaches its room: drawn again the same frame.
        app.world_mut()
            .entity_mut(inst)
            .get_mut::<crate::wmo_portal::WmoPortalInstance>()
            .unwrap()
            .visible[0] = true;
        app.update();
        assert_eq!(
            *app.world().entity(body).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "PVS reach ⇒ drawn"
        );
        // And a lit room does not exempt a member from the view: the sibling producer
        // frustum-tests members too. Move the body behind the camera — hidden again.
        *app.world_mut()
            .entity_mut(body)
            .get_mut::<GlobalTransform>()
            .unwrap() = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 50.0));
        app.update();
        assert_eq!(
            *app.world().entity(body).get::<Visibility>().unwrap(),
            Visibility::Hidden,
            "a visible room's behind-camera member is still pass 2"
        );
    }

    /// A doorway on the right admits the body on the right and rejects the one on the left — the
    /// same law as the terrain test above, now on a whole object rather than a chunk of ground.
    #[test]
    fn a_doorway_admits_only_its_own_side() {
        let (vis, verdict) = run_bodies(
            ExteriorWindows::Windows(vec![[0.1, -1.0, 1.0, 1.0]]),
            &[
                (Vec3::new(40.0, 0.0, -50.0), outdoors(), creature_box()),
                (Vec3::new(-40.0, 0.0, -50.0), outdoors(), creature_box()),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Inherited, Visibility::Hidden],
            "the window is on the right"
        );
        assert_eq!((verdict.bodies, verdict.bodies_hidden), (2, 1));
    }

    /// One `apply_exterior_cull` run over a scene of **liquid surfaces** — the ADT spawn's own
    /// shape: a world root at `IDENTITY` (MCLQ positions are absolute) carrying a world-space
    /// `Aabb`, the `LiquidSurface` marker and the `ExteriorScene` tag.
    fn run_liquid(
        windows: ExteriorWindows,
        surfaces: &[Aabb],
    ) -> (Vec<Visibility>, ExteriorCullVerdict) {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            .insert_resource(windows)
            .add_systems(Update, apply_exterior_cull);
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_2,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 1000.0,
                ..default()
            }),
        ));
        let ids: Vec<Entity> = surfaces
            .iter()
            .map(|bound| {
                app.world_mut()
                    .spawn((
                        GlobalTransform::IDENTITY,
                        *bound,
                        ExteriorScene,
                        crate::liquid::LiquidSurface,
                        Visibility::Inherited,
                    ))
                    .id()
            })
            .collect();
        app.update();
        let vis = ids
            .iter()
            .map(|e| *app.world().entity(*e).get::<Visibility>().unwrap())
            .collect();
        (vis, *app.world().resource::<ExteriorCullVerdict>())
    }

    /// One MCNK liquid layer's world box, centred at `at` — a flat 33.333 yd sheet, which is the
    /// granularity `spawn_liquids` actually produces (one entity per `LiquidMesh`, and an ADT chunk
    /// carries its liquid per chunk). Flat on purpose: a lake is a sheet, and the whole question is
    /// whether a sheet *overhead* reaches the doorway's part of the view.
    fn lake(at: Vec3) -> Aabb {
        const CELL: f32 = 33.333 / 2.0;
        Aabb::from_min_max(
            at - Vec3::new(CELL, 0.05, CELL),
            at + Vec3::new(CELL, 0.05, CELL),
        )
    }

    /// **The director's report, as a test**: from inside a cavern you could see the
    /// lake above through the ceiling. Sealed room ⇒ no windows ⇒ the reference's per-window
    /// exterior populate never runs, and its ADT liquid producer `0x683ab0` is reachable from
    /// nowhere else — so the lake is not submitted at all.
    ///
    /// Before 1652 this asserted nothing, because an ADT surface carried no `ExteriorScene` and the
    /// cull never reached it. That is what `liquid`/`liquid_hidden` exist to make visible: the
    /// counters are the difference between "the cull admitted the lake" and "the cull never saw
    /// it", which from a screenshot are the same picture.
    #[test]
    fn a_sealed_room_hides_the_lake_overhead() {
        let overhead = Vec3::new(0.0, 30.0, -60.0);
        let (vis, verdict) = run_liquid(ExteriorWindows::Windows(Vec::new()), &[lake(overhead)]);
        assert_eq!(
            vis,
            vec![Visibility::Hidden],
            "a sealed cavern draws no exterior liquid — the ceiling is not a depth test"
        );
        assert_eq!(
            (verdict.liquid, verdict.liquid_hidden),
            (1, 1),
            "and the cull must have REACHED it — (0, 0) here is the pre-1652 defect, which \
             leaves the surface `Inherited` and looks identical on screen to being admitted"
        );
    }

    /// The control that must not change: **outdoors nothing about liquid moves.** The driver's
    /// outside leg is one full-screen window (`0x6811ca`), so the gate is `Open` and every surface
    /// goes back to whatever else owns it — which, for liquid, is nothing. If this ever fails, the
    /// lake has started disappearing in the open world, which is a far worse bug than the one 1652
    /// fixed.
    #[test]
    fn outdoors_every_lake_is_admitted() {
        let (vis, verdict) = run_liquid(
            ExteriorWindows::Unrestricted,
            &[
                lake(Vec3::new(0.0, 30.0, -60.0)),
                lake(Vec3::new(0.0, -2.0, 400.0)), // behind the eye: still not this system's call
            ],
        );
        assert_eq!(vis, vec![Visibility::Inherited; 2]);
        assert_eq!((verdict.liquid, verdict.liquid_hidden), (2, 0));
    }

    /// A doorway shows the water on its own side of the view and no other — the same law the
    /// terrain and body legs take, on the bucket that was missing it. Both sheets sit at the same
    /// height and depth; the only difference is which way the doorway faces.
    #[test]
    fn a_doorway_admits_only_the_water_on_its_own_side() {
        let (vis, verdict) = run_liquid(
            ExteriorWindows::Windows(vec![[0.1, -1.0, 1.0, 1.0]]),
            &[
                lake(Vec3::new(60.0, 10.0, -60.0)),
                lake(Vec3::new(-60.0, 10.0, -60.0)),
            ],
        );
        assert_eq!(
            vis,
            vec![Visibility::Inherited, Visibility::Hidden],
            "the window is on the right"
        );
        assert_eq!((verdict.liquid, verdict.liquid_hidden), (2, 1));
    }

    /// **The failure mode this change could have had, pinned:** the cavern's OWN pool must not be
    /// blanked by the sealed room it sits in. A WMO pool carries `WmoGroupVis`, so it is
    /// owned by the model-visibility authority — which exempts the camera's own placement and folds
    /// the window term in itself — and this system's `Without<WmoGroupVis>` filter must keep
    /// its hands off it entirely, tag or no tag.
    ///
    /// Asserting on `liquid` is the point: the counter proves the surface was not merely *admitted*
    /// but never walked, which is the difference between the two authorities agreeing and one of
    /// them silently overwriting the other.
    #[test]
    fn a_wmo_pool_is_never_written_by_this_system() {
        let mut app = App::new();
        app.init_resource::<ExteriorCullVerdict>()
            .insert_resource(Assets::<benilla_assets::WmoModel>::default())
            // The sealed room — the one case that would blank it.
            .insert_resource(ExteriorWindows::Windows(Vec::new()))
            .add_systems(Update, apply_exterior_cull);
        let instance = app.world_mut().spawn(()).id();
        let pool = app
            .world_mut()
            .spawn((
                GlobalTransform::IDENTITY,
                lake(Vec3::new(0.0, 0.0, -10.0)),
                ExteriorScene,
                crate::liquid::LiquidSurface,
                crate::wmo_portal::WmoGroupVis {
                    instance,
                    groups: std::sync::Arc::from([0u16].as_slice()),
                },
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().entity(pool).get::<Visibility>().unwrap(),
            Visibility::Inherited,
            "the pool in the room you are standing in belongs to the model-visibility authority"
        );
        let verdict = *app.world().resource::<ExteriorCullVerdict>();
        assert_eq!(
            (verdict.tested, verdict.liquid),
            (0, 0),
            "`Without<WmoGroupVis>` must exclude it from the walk, not just from the verdict"
        );
    }
}
