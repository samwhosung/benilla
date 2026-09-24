//! WMO portal-based visibility culling — the per-frame pass that decides which of a building's groups
//! are reachable through portals from the camera's current group, so an interior the camera can't see
//! through any doorway (the Stormwind cathedral above the Trade District) is not drawn.
//!
//! This is the **faithful** mechanism, re-derived from `WoW.exe` 5875. The client's recursion
//! (`0x6b41c0`) carries a 2-D screen rectangle as its working frustum and floods the portal graph
//! from the camera's group: for each portal it (1) tests the camera is on the portal's front side,
//! (2) projects the portal polygon to a screen rect, (3) intersects that with the incoming rect,
//! and (4) recurses into the neighbour with the narrowed rect — terminating a branch the moment the
//! rect collapses to zero area. That collapse is exactly why the cathedral culls from the Trade
//! District but draws from the gates.
//!
//! Exterior shells: a building's outer-shell group (an inn's walls/roof) often carries **no
//! portals**, so the interior flood can't reach it. The client draws exterior groups from inside
//! via a separate *deferred-window* pass, **gated** on the flood having reached a doorway onto the
//! outdoors (the deferred-window count `0xcbe320`; empty skips it, `0x6b3c87`): reach none and the
//! whole pass is skipped — you can't see out. Reach one and, **per window**, every `0x8` group
//! visible in a frustum clipped to that doorway becomes a **flood root of its own** (`0x6b3d39 call
//! 0x6b41c0` — the same recursion, not a draw), carrying that window's rect onward through the
//! graph. So a doorway shows you the courtyard beyond it *and* what the courtyard's own portals
//! show, narrowing all the way (decision 1853).
//!
//! Seam: this module only **computes** the per-group visible set ([`WmoPortalInstance::visible`]). The
//! single `Visibility` authority ([`crate::debug_panel`]'s `apply_model_visibility`, decision 0025)
//! reads it and ANDs it with the dev toggles + the far-clip cull — we never write `Visibility` here.
//!
//! Current-group seed: the **downward raycast** in [`seed`] — the byte-audited client mechanism
//! (`0x6821f0` → `0x6be250` → `0x6a3f80`): walking-collision faces race portal crossings under the
//! eye, and the verdict is a **seed set** (in-group + across-group), each flooded as an independent
//! root. The epsilons here (side test exactly `0.0`, rect collapse `0.001`, the
//! `w`-clamp pair, the `0.1` near-parallel snap) are the client's own constants, read from the binary
//! — do not "fix" them to tolerances without re-deriving there first.

use std::sync::Arc;

mod fog;
mod interior;
mod probe;
mod room_vis;
mod seed;

use fog::select_wmo_fog;
pub use fog::{CameraWmoFog, WmoFogTarget};
pub use interior::{
    indoor_verdict_at, indoors_at, terrain_z_local, IndoorVerdict, LightAttach,
    INTERIOR_PROBE_HEIGHT,
};
pub(crate) use interior::{surface_terrain_sample, WmoAreas, POSITION_PROBE_LIFT};
use interior::{track_area_interior, track_current_interior, track_unit_interiors};
pub use interior::{
    CurrentAreaInterior, CurrentWmoInterior, PlayerWmoRoom, UnitWmoRoom, WmoInteriorKeys,
};
use probe::TraceLog;
pub use probe::WmoCullProbe;
pub use room_vis::room_pvs_visible;
use seed::dominant_axes;
pub use seed::{down_ray_seeds, floor_z_at, DownRaySeeds};

use crate::terrain_stream::{terrain_height_under, TerrainStreamer};
use crate::view::WorldCamera;
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::{AdtTile, WmoGroupNav, WmoModel, WmoPortalInfo};
use bevy::camera::primitives::{Aabb, Frustum};
use bevy::camera::Projection;
use bevy::math::{Affine3A, Vec4};
use bevy::prelude::*;

/// MOGP EXTERIOR flag — an "outdoor" group. The flood defers these to the open world rather than
/// treating them as portal-gated interiors (and seeds the outside leg from them).
const EXTERIOR: u32 = 0x8;

/// MOGP EXTERIOR_LIT flag — an interior-graph group lit as OUTDOORS. The unit light classify
/// (`0x6a87f0`) forks its LIGHTING class on `MOGI & 0x48` — `0x8` **or** `0x40` → the exterior leg
/// (`or [node+0xc],0x4`: sun diffuse × the MCSH-driven 2.5/0.5 intensity, scene fog) — while the
/// zone-text indoor bit (`[node+0x90]` bit 0) keys on `0x8` alone, so a `0x40`-only group is
/// "indoors" for area naming but sunlit. The city street WMOs are exactly this: Stormwind's
/// streets (115 of 306 groups) and Orgrimmar's valleys are `0x40`-without-`0x8` (decision 0475).
const EXTERIOR_LIT: u32 = 0x40;

/// Eye-on-portal-plane band (WMO yards): an eye within this of a portal's plane **and inside its
/// polygon** gets the full-screen rect for that portal (the client's `0x6b46f0` special case, band
/// `|d| <= 0.01` at `0x8029d0`). The side test still runs — the recorded "skip the side test" flag is
/// dead code in the client (`0x6b43c9`).
const ON_PLANE_EPS: f32 = 0.01;
/// The `|w|` band below which a clip-space vertex's `w` is substituted before the perspective divide —
/// the client's `0x801360` (`0.001`), strict `<`.
const W_CLAMP_BAND: f32 = 0.001;
/// The substituted `w` — **positive regardless of the vertex's sign** (the client's `1e-5` immediate
/// `0x3727c5ac`). A vertex with `w <= -0.001` is NOT clamped: it divides by its real negative `w`.
const W_CLAMP_SUB: f32 = 1.0e-5;
/// Minimum NDC extent for a narrowed screen rect to count as non-empty — the client's zero-area eps
/// `0x801360` (`0.001`); a collapsed window kills the branch.
const RECT_EPS: f32 = 0.001;
/// Ray-vs-plane near-parallel threshold on the denominator (the client's f64 `0x811658` = `0.0001`):
/// below it, the down-ray is parallel to the portal plane and the crossing exists only via the snap.
const PORTAL_NEAR_PARALLEL: f32 = 1.0e-4;
/// The "eye embedded in the plane" snap window (yd) for a near-parallel portal (the client's `0.1`
/// immediate pushed at `0x6a40c0`): a vertical doorway counts as crossed by the vertical down-ray only
/// while the eye is within this of its plane.
const PORTAL_PLANE_SNAP: f32 = 0.1;
/// Nearest-crossing accept eps for the portal leg (the client's `0x80c4f4` ≈ `1e-4`): a portal crossing
/// within this of the running nearest face hit still wins — a floor-hole portal coincident with the
/// slab's collision faces flips the room at the plane.
const NEAREST_TIE_EPS: f32 = 1.0e-4;
/// MOGI/MOGP `0x10000` — the **callback-pass** group. The reference draws these in a third pass of
/// its own (`0x6b3d6f..0x6b3dbc`), against the FULL camera frustum and gated on nothing, and Pass 2
/// explicitly skips them (`0x6b3d0b`). Both passes read the bit off MOGI; benilla's
/// [`WmoGroupNav::flags`] is MOGP, a strict SUPERSET of MOGI differing by exactly
/// `0x1|0x4|0x200|0x800|0x1000` in all 5219 shipped groups (the loader `0x6c4530`) — so the two
/// agree on this bit and on [`EXTERIOR`], which is why reading MOGP here is safe.
///
/// 24 groups in 1.12 carry it: 14 in `stormwind.wmo` (the canals and the tall district shell),
/// 6 across the two Dire Maul roots, and the 4 Razorfen enclosing shells that also carry `0x8`.
const CALLBACK_PASS: u32 = 0x10000;
/// The client's deferred-window worklist is a **fixed 16 records with no bound check** (`0x6b37b0`);
/// nothing in the shipped data comes near it. We do not reproduce the overflow — emulating a buffer
/// overrun is not fidelity — but we do watch the number, because a flood producing more windows than
/// the reference's own designers left room for is far more likely to be OUR bug than the data's
/// (decision 1148: one Trade District room once opened eleven windows onto Elwynn).
const WORKLIST_CAP: usize = 16;
/// Recursion depth cap — the client's own, `[0xcb004c]` seeded from `[0x86b6b8]` = **10**, refreshed
/// by every `depth = 0` entry (so Pass 2's re-seeds each get a fresh ten hops, RE 2026-09-02). Ours
/// was a guessed backstop of 64 until the byte read; dropping it to the real number changes nothing
/// measurable in any audited building — Darnassus's walk-on gain is identical at 10 and 64, and all
/// 13 real-data sweeps are unmoved — which is the answer you want from a cap: it is a cycle
/// backstop, not a visibility term.
const DEPTH_CAP: u32 = 10;
/// Hard cap on flood iterations — a runaway-graph backstop; real WMOs settle far below this.
const MAX_ITERS: u32 = 1 << 16;

/// An NDC axis-aligned rectangle `[min_x, min_y, max_x, max_y]` — the "screen frustum" carried through
/// the flood. The full screen is `[-1, -1, 1, 1]`.
pub(crate) type Rect = [f32; 4];
const FULL_SCREEN: Rect = [-1.0, -1.0, 1.0, 1.0];

/// The **deferred exterior-window worklist** — the doorways through which the outdoor world is
/// allowed to draw this frame, as NDC rects ([`Rect`]).
///
/// This is the client's `[0xcbe324]` list (count `[0xcbe320]`, stride `0x14` =
/// `{x0, y0, x1, y1, portalMaxPlaneDist}` in 0..1 screen space), which the world-scene driver
/// `0x681070` copies into `0xc7cb7c` and then walks **once per window**, with the frustum narrowed to
/// that window's rect. `0x682fa0` is the only producer for every exterior bucket, so no window in
/// frame means no exterior content at all (`0x681199 jbe 0x681204` skips the whole walk on count 0).
///
/// [`Self::Unrestricted`] is the driver's **outside leg** (`0x6811ca`): with no containing WMO the
/// client runs one populate walk against the literal full-screen rect, i.e. the ordinary view frustum.
#[derive(Resource, Default, Clone, PartialEq, Debug)]
pub enum ExteriorWindows {
    /// The camera is not inside a WMO interior — the whole frustum is the window.
    #[default]
    Unrestricted,
    /// The camera is inside. Exterior content draws only where it survives one of these sub-frusta;
    /// **empty means nothing exterior draws**, which is the sealed-room case and is correct.
    Windows(Vec<Rect>),
}

/// Static tag on every portal-culled WMO piece — a group render submesh, a group's MLIQ surface, a
/// MODD prop — naming which placement instance it belongs to and which absolute group indices can
/// draw it, so the visibility authority can look up their per-frame PVS bits.
///
/// Geometry and liquid belong to exactly one group. A **prop** can be named by many: the reference
/// draws doodads per *visible* group out of that group's own MODR, so every referrer is an
/// independent chance to be drawn (see [`Self::drawn_by`]). Built once per group/prop at spawn, so
/// the `Arc` is one allocation per key, not one per submesh.
#[derive(Component, Clone)]
pub struct WmoGroupVis {
    pub(crate) instance: Entity,
    pub(crate) groups: Arc<[u16]>,
}

impl WmoGroupVis {
    /// The portal verdict: drawn iff **any** referencing group is in this frame's PVS. An index
    /// past the instance's visible set (a stale or short set) reads as visible — the same fail-open
    /// the rest of the cull takes, so a lookup miss can never blank a building.
    pub fn drawn_by(&self, inst: &WmoPortalInstance) -> bool {
        self.groups
            .iter()
            .any(|&g| inst.visible.get(g as usize).copied().unwrap_or(true))
    }

    /// The interior-fog verdict: this piece takes the building's MFOG triple iff **any** naming
    /// group does ([`WmoPortalInstance::interior_fog`], the client's `[0xca7f00]`). ORed for the
    /// same reason `drawn_by` ORs — a prop several rooms name is drawn from any of them, so it
    /// wears the fog of any of them. A lookup miss reads FALSE, the opposite of `drawn_by`'s
    /// fail-open: a missing bit here must not paint a room's fog onto the open world, and the scene
    /// fog is what every un-gated surface already wears.
    pub fn interior_fogged_by(&self, inst: &WmoPortalInstance) -> bool {
        self.groups.iter().any(|&g| inst.fogs_group(g))
    }
}

/// **Is a room-bound RIDER drawn this frame?** — [`WmoGroupVis::drawn_by`] resolved for the lanes
/// that carry their rooms *by value* rather than as a component, because their entity is not a
/// model submesh: a prop's particle clouds and ribbon trails
/// ([`crate::particles::EmitterFade::room_admitted`]) and a building's point-light sources
/// ([`crate::lighting::LightRooms`]).
///
/// Every one of them exists for the same reason. The reference instantiates a WMO's furniture out
/// of each **visible** group's own MODR list (`0x695aa0` from the visible-group walk `0x698720`,
/// decision 0689), so a prop in a culled room is never created and has nothing to tick, draw or
/// light with. We create props once and cull them per frame, so each rider has to ask for itself —
/// and this is where the question is spelled, once, for all of them.
///
/// - **No rooms** — admitted. An ADT map doodad, a creature, a held item: nothing claims it.
/// - **Rooms, placement resident** — `drawn_by`: the identical predicate the owner's own submeshes
///   are culled by, so a model and its riders can never disagree about which frame they are in.
/// - **Rooms, placement gone** — refused, where `drawn_by` itself fails OPEN. Deliberate, and the
///   one asymmetric arm: a rider outlives its building by the frame between the placement
///   despawning and the cascade reaching it, and one frame of an additive strip — or a torch —
///   hanging where a building used to be is a bright artefact, where one frame of a missing one is
///   nothing at all. [`crate::skybox`]'s skybox resolve refuses on the same asymmetry, and 1276 is
///   the general rule: an unknown with a testable answer must be tested, but where the error
///   directions are lopsided, take the cheap one.
pub fn room_admits(room: Option<&WmoGroupVis>, instance: Option<&WmoPortalInstance>) -> bool {
    match (room, instance) {
        (None, _) => true,
        (Some(r), Some(inst)) => r.drawn_by(inst),
        (Some(_), None) => false,
    }
}

/// One placed WMO building's portal-cull state. Spawned alongside the building's geometry; despawns
/// with the placement (it's pushed into the placement's entity list).
#[derive(Component)]
pub struct WmoPortalInstance {
    /// The building asset (portal graph + per-group nav).
    pub handle: Handle<WmoModel>,
    /// Placement transform as Bevy **model→world**. A portal vertex is WoW model space, so it reaches
    /// world via `world_from_local * wow_to_bevy(v)` — the same path `build_submesh_mesh` bakes the
    /// rendered geometry through, so portals and walls live in one space.
    pub world_from_local: Affine3A,
    /// The placement's `MODF.nameSet` — the `WMOAreaTable.NameSetID` audio/naming variant
    /// (one abbey model serves Northshire and Tyr's Hand by name set).
    pub name_set: u16,
    /// Per-group visibility (indexed by absolute group index) for this frame; `true` = in the portal
    /// PVS. Seeded all-`true` (everything shows until the first compute, and forever for portal-less
    /// props).
    pub visible: Vec<bool>,
    /// Per-group **interior-fog** gate for this frame (the client's `[0xca7f00]`): `true` = this
    /// group's surfaces and its doodads draw with the building's MFOG triple, `false` = they
    /// inherit the scene fog. Computed by the same flood as `visible` — see [`GroupPvs`] for the
    /// rule and the byte cites. Seeded all-`false`: nothing wears a room's fog until a flood says
    /// so, and a portal-less prop (which never floods) never does.
    pub interior_fog: Vec<bool>,
    /// Per-group **ever-visited** latch — the client's render-record persistence: the visit
    /// callback `0x685d70` creates a per-(instance,group) record the first time the flood visits
    /// a group, the record pools are cleared only at world init/teardown, and a recorded MLIQ
    /// group's **liquid surface draws every frame with no portal/frustum re-check** (`0xc7cb04`
    /// list, walk `0x684cd0 → 0x6b62e0`). So liquid is gated on *ever seen*, not on this frame's
    /// PVS — the Great Forge's
    /// walkway-level pool must not vanish when its group drops out of the flood. Latched by
    /// [`compute_wmo_pvs`] from `visible`; starts all-`false` (a never-visited group's liquid is
    /// genuinely absent in the reference until first visit). Lifetime is the placement's residency,
    /// not the world session — a restreamed placement re-latches from the first flood, a small
    /// divergence from the client's global pools noted in the B65 decision record.
    pub liquid_visited: Vec<bool>,
    /// Per-group **whole-group submersion** (indexed by absolute group index), from the group's
    /// MOGP `groupLiquid` override: `Some(kind)` means standing anywhere inside this group is
    /// standing under liquid — at every Z, with no surface grid involved at all.
    ///
    /// Baked at spawn rather than resolved per query because it is a property of the *room*, and
    /// the room is already what the liquid query carries ([`crate::liquid::LiquidClaim`]). Empty on
    /// a placement spawned before its model resolved.
    pub flooded: Vec<Option<benilla_formats::LiquidKind>>,
}

impl WmoPortalInstance {
    /// **Is one group on this frame's interior-fog chain?** — the client's `[0xca7f00]` read for a
    /// single group, which is the grain every consumer of the gate actually asks at: a WMO piece
    /// ORs it over the groups that name it ([`WmoGroupVis::interior_fogged_by`]), and a unit asks
    /// it of the one room its light node attached to (`crate::interior`, the `[P+0x98]` conjunct
    /// of decision 1792 §4). A group past the set reads FALSE — see `interior_fogged_by` for why
    /// this one fails closed where `drawn_by` fails open.
    pub fn fogs_group(&self, group: u16) -> bool {
        self.interior_fog
            .get(group as usize)
            .copied()
            .unwrap_or(false)
    }
}

/// One WMO **room**: a placed building's instance entity plus the absolute group index inside it.
///
/// This is the client's `[0xc7b748]` (the camera-containing placed map-object) paired with the
/// current group index its visible-group set `0xc7cd88` carries. It is the identity every "which
/// room is this subject in" answer resolves to,
/// and — since decision 0696 — the **scope key of the liquid query**: a building's pool belongs to a
/// room, so only a subject standing in that placement can be in it. A liquid footprint has no floor,
/// so without an owner a pool claims every position under its XY forever, in any building, at any
/// depth (Uldaman read as submerged under a *mushroom cave's* water 186 yd overhead).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WmoRoom {
    /// The placement's [`WmoPortalInstance`] entity.
    pub instance: Entity,
    /// Absolute group index within the building.
    pub group: u16,
}

/// The camera's PVS-flood interior claim this frame — `None` with the camera over open world or an
/// EXTERIOR (`0x8`) group's floor; an EXTERIOR_LIT-only porch (`0x40`) still claims (the
/// `[0xc7b748]` writer's 0x8-only rejection, `0x6be451`). Written by [`compute_wmo_pvs`] off the
/// same down-ray seed the flood runs; consumed by the weather-visible gate (`weather::precip`) and
/// by the camera-eye submersion verdict (`liquid::detect_submersion` — the reference's per-frame
/// environment probe `0x6809c0` picks the WMO group's MLIQ over the ADT liquid on exactly this
/// global).
///
/// When `Some`, `exterior_visible` reports whether this placement's flood reached an
/// EXTERIOR-flagged group (`flags & 0x148`) through a screen-rect-surviving portal window — the
/// pass-2 law of the weather flag `[0xca80c4]` (`0x6b42d9`): rain stays visible — and falling —
/// through a doorway the camera can SEE; face away and the window clips out, killing the draw that
/// frame (view-dependent by design).
#[derive(Resource, Default, Clone, Copy, PartialEq)]
pub struct CameraInteriorClaim(pub Option<InteriorClaim>);

/// See [`CameraInteriorClaim`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InteriorClaim {
    /// The room the camera EYE is in — the placement + its current group.
    pub room: WmoRoom,
    /// An EXTERIOR-flagged group (`flags & 0x148`) is in the claiming placement's PVS.
    pub(crate) exterior_visible: bool,
}

/// Ordering handle so the `Visibility` authority can run **after** the PVS is computed this frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WmoPvsSet;

/// Registers the per-frame PVS compute. The apply side lives in [`crate::debug_panel`].
pub struct WmoPortalPlugin;

impl Plugin for WmoPortalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            interior::load_wmo_areas.after(benilla_assets::AssetSet::Open),
        )
        .init_resource::<CurrentWmoInterior>()
        .init_resource::<PlayerWmoRoom>()
        .init_resource::<CurrentAreaInterior>()
        .init_resource::<CameraWmoFog>()
        .init_resource::<CameraInteriorClaim>()
        .init_resource::<ExteriorWindows>()
        .init_resource::<WmoCullProbe>()
        .add_systems(
            Update,
            (
                compute_wmo_pvs,
                track_current_interior,
                track_area_interior,
                track_unit_interiors,
            )
                .in_set(WmoPvsSet)
                // **On this frame's camera pose, not last frame's** (decision 1503). The flood,
                // the interior claim and the exterior windows all key on the eye, and every one of
                // them decides what the frame is allowed to draw. Unordered, this set could even
                // run before the controller had moved the camera at all; ordered here it answers
                // about the eye the frame will be rendered from.
                .after(crate::view::CameraPoseSet),
        );
    }
}

/// Max drop (yd) for the current-group down-ray: a floor more than this far below the eye doesn't count
/// as "the room you're in" (you're over open space ⇒ outside). Matches the client's ray length (`1760`).
const MAX_FLOOR_DROP: f32 = 1760.0;

/// Recompute each resident WMO's per-group visible set from the camera. Cheap: a handful of buildings,
/// each a small portal flood; a portal-less prop just stays all-visible.
fn compute_wmo_pvs(
    wmos: Res<Assets<WmoModel>>,
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    mut instances: Query<(Entity, &mut WmoPortalInstance)>,
    streamer: Res<TerrainStreamer>,
    adt_tiles: Res<Assets<AdtTile>>,
    mut probe: ResMut<WmoCullProbe>,
    mut camera_fog: ResMut<CameraWmoFog>,
    mut camera_claim: ResMut<CameraInteriorClaim>,
    mut camera_windows: ResMut<ExteriorWindows>,
) {
    // The world Camera3d (the egui overlay is a Camera2d). None before it spawns ⇒ leave last frame's
    // sets untouched (everything was visible, which is the safe default).
    let Some((cam_t, proj)) = cam.iter().next() else {
        return;
    };
    let clip_from_world = proj.get_clip_from_view() * cam_t.to_matrix().inverse();
    let eye_world = cam_t.translation();
    // Publish the pose this authority ran on (see [`WmoCullProbe::eye`]).
    probe.eye = eye_world;
    // The terrain leg of the seed's down-ray, sampled once for the camera's column: the client casts the
    // same segment at the ground and drops the WMO hit when the ground is nearer (`FUN_006821f0`).
    let terrain = terrain_height_under(&streamer, &adt_tiles, eye_world);
    // `WOW_CULLDUMP=<path>` re-requests the dump every frame (last writer wins): a headless
    // capture has no panel button, and the first frames race asset residency, so a one-shot
    // request from startup would photograph an empty world. A path, `WOW_MOVE_TRACE`'s shape —
    // never a default this crate picks; [`WmoCullProbe::dump_to`] says why. (`WOW_CULLDUMP=1` is
    // no longer a spelling: it wrote `target/wmo-cull-trace.txt` into whatever the cwd was.)
    static ENV_DUMP: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    let dump_to = probe.dump_to.take().or_else(|| {
        ENV_DUMP
            .get_or_init(|| std::env::var_os("WOW_CULLDUMP").map(std::path::PathBuf::from))
            .clone()
    });
    // The camera pose IN FULL: replaying a dump in the pin probe needs forward + fov + aspect, not
    // just the eye — B65's first dump recorded only the eye and the replay had to reconstruct the
    // look from the player's position.
    let mut dump_text = dump_to.is_some().then(|| {
        let fwd = cam_t.forward();
        let (fovy, aspect) = match proj {
            Projection::Perspective(p) => (p.fov, p.aspect_ratio),
            _ => (f32::NAN, f32::NAN),
        };
        format!("eye world: {eye_world:?}\nforward: {fwd:?} fovy {fovy:.4} aspect {aspect:.4}\n")
    });

    // The camera's MFOG fog target + interior claim — resolved from the same down-ray seed the
    // flood runs anyway (see [`fog`]); the first placement whose seed claims a TRUE INTERIOR
    // group wins both.
    let mut fog_target: Option<WmoFogTarget> = None;
    let mut claim: Option<InteriorClaim> = None;
    // `None` here = the driver's outside leg (no containing map object): one full-screen walk.
    let mut windows: Option<Vec<Rect>> = None;
    for (entity, mut inst) in &mut instances {
        let Some(model) = wmos.get(&inst.handle) else {
            continue;
        };
        let groups = model.group_nav.len();
        // A WMO with no portal graph (single-group props, doors) is never culled — keep all groups on.
        // Written through the change gate below like the flood's own answer: this branch used to
        // deref-mut every portal-less prop every frame, which marked the whole population changed
        // and would defeat any future `Changed<WmoPortalInstance>` run condition (the 1364 item-2
        // precondition) before it was born.
        if model.portal_refs.is_empty() || model.portal_infos.is_empty() {
            let held = inst.bypass_change_detection();
            // …and never on the interior fog lane: a portal-less prop runs no flood, so there is
            // no chain to be on (the same reading the fog resolve takes — see [`fog`]'s module doc).
            let mut changed = false;
            if held.visible.len() != groups || held.visible.iter().any(|v| !*v) {
                held.visible = vec![true; groups];
                changed = true;
            }
            if held.interior_fog.len() != groups || held.interior_fog.iter().any(|v| *v) {
                held.interior_fog = vec![false; groups];
                changed = true;
            }
            if changed {
                inst.set_changed();
            }
            continue;
        }
        let world_from_local = inst.world_from_local;
        // The eye in this WMO's model space (WoW axes): the down-ray origin (seed) and the portal
        // front-side test point. The down-ray from the camera is robust on its own — a collision-shoved
        // camera still drops onto the floor beneath it — so there's no player-position seed any more.
        let local_from_world = world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye_world));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye_world, z));
        // ONE flood, with whichever recorders are attached — the dump must never change the answer
        // it is photographing. It used to be an `if dump { trace } else { tap }`, so `WOW_CULLDUMP=1`
        // skipped the claim/fog/window computation outright and every dumped run reported
        // `room=none windows=unrestricted` no matter where the camera stood; the trace file said
        // `seed: in Some(102)` on the same frame. That cost a wrong reading while diagnosing 1148.
        let mut tap = SeedTap::default();
        let mut log = dump_text
            .is_some()
            .then(|| TraceLog::new(model, eye_local, terrain_local));
        // Compare-then-write (1362's shape): a parked camera's flood answers the same set every
        // frame, and writing it anyway re-marked every instance changed — see the branch above.
        let mut fresh = compute_pvs_traced(
            model,
            eye_local,
            terrain_local,
            &clip_from_world,
            &world_from_local,
            &mut (&mut tap, &mut log),
        );
        if inst.bypass_change_detection().visible != fresh.visible {
            inst.bypass_change_detection().visible = fresh.visible;
            inst.set_changed();
        }
        if inst.bypass_change_detection().interior_fog != fresh.interior_fog {
            inst.bypass_change_detection().interior_fog = fresh.interior_fog;
            inst.set_changed();
        }
        if let (Some(text), Some(log)) = (&mut dump_text, &log) {
            // Which building this trace belongs to — the dump lists every placement in range, and
            // an id alone cannot tell two night-elf houses apart. The origin can.
            let o = world_from_local.translation;
            text.push_str(&format!(
                "placement @ world ({:.1}, {:.1}, {:.1}) — {} deferred window(s) {:?}, {} walk steps\n",
                o.x,
                o.y,
                o.z,
                fresh.windows.len(),
                fresh.windows,
                fresh.iters
            ));
            text.push_str(&log.text);
            text.push_str(&format!("visible: {:?}\n\n", inst.visible));
        }
        // Interior-fog + weather claims off the seed already computed. The claim mask is bit 0x8
        // ALONE (`[0xc7b748]` writer `0x6be451/77` tests only EXTERIOR): an EXTERIOR_LIT-only
        // porch/courtyard (`0x40`) still claims the camera and engages this WMO's MFOG fog; the
        // weather gate reads whether the placement's flood reached an exterior group (already
        // view-clipped — the deferred-window gate fires only when the doorway's screen rect
        // survives, the pass-1 law).
        if claim.is_none() {
            if let Some(gi) = tap.in_group {
                if model
                    .group_nav
                    .get(gi)
                    .is_some_and(|n| n.flags & EXTERIOR == 0)
                {
                    // **The MFOG engagement conjunct** (the byteOut loop `0x69de5f`–`0x69dea0` over
                    // the camera's containing set `0xc7cd88`): the interior fog target exists only
                    // when one of the camera's ≤2 CONTAINING groups — the down-ray's in-group and
                    // its across-portal partner, which is exactly that set — is a TRUE interior,
                    // `flags & 0x48 == 0`. In an exterior-LIT courtyard the client's `t` decays to
                    // 0 and the interior triple
                    // equals the scene triple; no surface of the building is on the lane either
                    // (decision 1787), so what this conjunct actually decides is the fog of an
                    // interior-classified UNIT standing in the building, which reads the triple by
                    // its own classification and would otherwise wear a courtyard's MFOG.
                    //
                    // The candidate walk reads THAT group's own MOGP fog indices (`grp+0x34`), not
                    // the containing group's unconditionally — a courtyard's indices never engage.
                    // The interior CLAIM below keeps the `0x8`-alone test it has always had: the
                    // `[0xc7b748]` writer rejects on EXTERIOR only (`0x6be451`), and the weather
                    // gate and the submersion probe both read the claim, not this.
                    fog_target = [Some(gi), tap.across]
                        .into_iter()
                        .flatten()
                        .find_map(|g| {
                            model
                                .group_nav
                                .get(g)
                                .filter(|n| n.flags & (EXTERIOR | EXTERIOR_LIT) == 0)
                        })
                        .and_then(|n| select_wmo_fog(&model.fogs, n.fog_indices, eye_local));
                    claim = Some(InteriorClaim {
                        room: WmoRoom {
                            instance: entity,
                            group: gi as u16,
                        },
                        exterior_visible: model
                            .group_nav
                            .iter()
                            .zip(&inst.visible)
                            .any(|(n, v)| *v && n.flags & 0x148 != 0),
                    });
                    // The deferred exterior-window list is this claiming placement's alone: the
                    // reference floods the CONTAINING map object (`[0xc7b748]`) and defers that
                    // flood's windows. It arrives already built — [`GroupPvs::windows`], the same
                    // list the flood's own Pass 2 tested this building's exterior groups against.
                    //
                    // The push test inside it is [`opens_a_window`] — `0x8` ALONE, NOT the `0x148`
                    // the `exterior_visible` line above reads. Those two are neighbours in the
                    // binary and neighbours here, and 0774 recorded them as *correlated, not
                    // equivalent*; the port read `0x148` anyway, which made every
                    // `0x40`-without-`0x8` group a doorway onto the open world. Stormwind's streets
                    // are 115 of its 306 groups (decision 0475), so one Trade District room opened
                    // eleven windows onto Elwynn — decision 1148.
                    windows = Some(std::mem::take(&mut fresh.windows));
                }
            }
        }
        // The render-record persistence latch (`liquid_visited` — the client's per-(instance,group)
        // visit records): a group entering the PVS this frame stays "visited" for the rest of this
        // placement's residency, and the visibility authority draws its MLIQ surface off THIS latch,
        // never off the per-frame `visible`.
        // Through the change gate like the flood's own answer above: this latch used to be
        // written through `Mut` for every visible building on every frame, which marked the
        // whole resident population changed and held `Changed<WmoPortalInstance>` open — the
        // one term of the visibility walk's still-frame skip (1979/1982) that a city can never
        // satisfy. A slot that actually flips is a PVS change and marks, as `visible` does.
        let held = inst.bypass_change_detection();
        let mut latched = false;
        if held.liquid_visited.len() != groups {
            held.liquid_visited = vec![false; groups];
            latched = true;
        }
        for g in 0..groups {
            if held.visible.get(g).copied().unwrap_or(false) && !held.liquid_visited[g] {
                held.liquid_visited[g] = true;
                latched = true;
            }
        }
        if latched {
            inst.set_changed();
        }
    }
    if camera_fog.0 != fog_target {
        camera_fog.0 = fog_target;
    }
    if camera_claim.0 != claim {
        camera_claim.0 = claim;
    }
    let want_windows = match windows {
        Some(rects) => ExteriorWindows::Windows(rects),
        None => ExteriorWindows::Unrestricted,
    };
    if *camera_windows != want_windows {
        *camera_windows = want_windows;
    }
    if let Some((mut text, path)) = dump_text.zip(dump_to) {
        // The frame's published verdict, after every placement has had its say: which room claimed
        // the camera and what the open world is allowed to draw through. The per-placement traces
        // above say what each flood found; only this says what the scene actually got.
        text.push_str(&format!("CAMERA CLAIM: {:?}\n", camera_claim.0));
        text.push_str(&format!("EXTERIOR WINDOWS: {:?}\n", *camera_windows));
        // The folder is made at the moment there is a trace to put in it (the stall sampler's
        // rule for `Diagnostics/`): an instrument never advertises itself with an empty folder.
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, &text) {
            Ok(()) => info!("wmo cull trace written to {}", path.display()),
            Err(e) => warn!("wmo cull trace: cannot write {}: {e}", path.display()),
        }
    }
}

/// The per-frame [`FloodTrace`] tap: the down-ray's in-group seed (the camera's current group), which
/// the interior-fog resolve reads — no duplicate down-ray.
///
/// The deferred windows used to be reconstructed here, by filtering every entered step for an
/// exterior destination. They are [`GroupPvs::windows`] now — the flood's own output — because the
/// flood's **Pass 2** needs the identical list to decide which exterior groups draw (decision 1826),
/// and two filters over the same steps are two places for the push test to drift.
#[derive(Default)]
struct SeedTap {
    in_group: Option<usize>,
    /// The down-ray's ACROSS-portal seed. With [`Self::in_group`] this is the client's
    /// `0xc7cd88` — the camera's ≤2 *containing* groups, which the MFOG engagement conjunct
    /// tests (not the flood's visible set).
    across: Option<usize>,
}
impl FloodTrace for SeedTap {
    fn seed(&mut self, seeds: DownRaySeeds) {
        self.in_group = seeds.in_group;
        self.across = seeds.across;
    }
}

/// **The deferred-window push test.** A flood step opens a window onto the OPEN WORLD exactly when
/// its DESTINATION group carries `0x8` — the recursion resolves the neighbour and tests `0x6b44f8
/// test byte [eax+0x10],0x8; 0x6b44fc je <skip-defer>` before pushing the clipped rect into
/// `[0xcbe324]`/`[0xcbe320]`. **`0xcbe310` is not the array**: the fill's `lea
/// edi,[4*ecx+0xcbe310]` is an addressing form whose addend is never dereferenced (record 0 is at
/// `0xcbe324`), and `0xcbe310` itself is the tail of the per-frame view-projection matrix
/// `[0xcbe2d8]+0x38` — not `0x6b4680`, the producer of the record's 5th float. Nothing else
/// qualifies: not [`EXTERIOR_LIT`], not the weather global's `0x148` (that mask lives at `0x6b42d0`
/// and drives `[0xca80c4]`, a *correlated but different* signal — decision 0774 said so and the
/// port read it anyway; decision 1148).
fn opens_a_window(nav: &[WmoGroupNav], to: usize) -> bool {
    nav.get(to).is_some_and(|n| n.flags & EXTERIOR != 0)
}

/// A tap on the flood's per-portal decisions. The `()` impl is a no-op the per-frame path uses; the
/// probe dump and the audit harness pass a real recorder — one flood implementation, no trace copy to
/// drift.
pub(crate) trait FloodTrace {
    fn seed(&mut self, _seeds: DownRaySeeds) {}
    fn side_fail(&mut self, _from: usize, _portal: u16, _to: usize, _d: f32) {}
    fn rect_none(&mut self, _from: usize, _portal: u16, _to: usize) {}
    fn rect_collapse(&mut self, _from: usize, _portal: u16, _to: usize) {}
    fn entered(&mut self, _from: usize, _portal: u16, _to: usize, _rect: Rect, _on_plane: bool) {}
    /// One Pass-2 verdict: an EXTERIOR group tested against the whole deferred worklist. Without
    /// this, "why is that wall still drawn?" has no reading at all — the flood's own hop lines
    /// never mention a group no portal reaches, which is exactly the population Pass 2 decides.
    fn pass2(&mut self, _group: usize, _flags: u32, _window: usize, _admitted: bool) {}
    /// One Pass-3 verdict: a `0x10000` callback-pass group against the full frustum.
    fn pass3(&mut self, _group: usize, _flags: u32, _admitted: bool) {}
    /// The flood produced more deferred windows than the reference's fixed worklist holds — a
    /// tripwire on OUR flood, not a client behaviour (see [`WORKLIST_CAP`]).
    fn worklist_over_cap(&mut self, _windows: usize) {}
}
impl FloodTrace for () {}

/// **Two recorders on one flood.** The per-frame path always needs the [`SeedTap`]; the dump wants a
/// [`TraceLog`] as well. Pairing them keeps that a single flood — the alternative (branch on "are we
/// dumping" and run one or the other) is what let `WOW_CULLDUMP=1` silently drop the claim/window
/// computation and report a room the same trace file contradicted.
impl<A: FloodTrace, B: FloodTrace> FloodTrace for (&mut A, &mut B) {
    fn seed(&mut self, seeds: DownRaySeeds) {
        self.0.seed(seeds);
        self.1.seed(seeds);
    }
    fn side_fail(&mut self, from: usize, portal: u16, to: usize, d: f32) {
        self.0.side_fail(from, portal, to, d);
        self.1.side_fail(from, portal, to, d);
    }
    fn rect_none(&mut self, from: usize, portal: u16, to: usize) {
        self.0.rect_none(from, portal, to);
        self.1.rect_none(from, portal, to);
    }
    fn rect_collapse(&mut self, from: usize, portal: u16, to: usize) {
        self.0.rect_collapse(from, portal, to);
        self.1.rect_collapse(from, portal, to);
    }
    fn entered(&mut self, from: usize, portal: u16, to: usize, rect: Rect, on_plane: bool) {
        self.0.entered(from, portal, to, rect, on_plane);
        self.1.entered(from, portal, to, rect, on_plane);
    }
    fn pass2(&mut self, group: usize, flags: u32, window: usize, admitted: bool) {
        self.0.pass2(group, flags, window, admitted);
        self.1.pass2(group, flags, window, admitted);
    }
    fn pass3(&mut self, group: usize, flags: u32, admitted: bool) {
        self.0.pass3(group, flags, admitted);
        self.1.pass3(group, flags, admitted);
    }
    fn worklist_over_cap(&mut self, windows: usize) {
        self.0.worklist_over_cap(windows);
        self.1.worklist_over_cap(windows);
    }
}

/// An absent recorder records nothing — so "attach the dump only when asked" costs no second flood.
impl<T: FloodTrace> FloodTrace for Option<T> {
    fn seed(&mut self, seeds: DownRaySeeds) {
        if let Some(t) = self {
            t.seed(seeds);
        }
    }
    fn side_fail(&mut self, from: usize, portal: u16, to: usize, d: f32) {
        if let Some(t) = self {
            t.side_fail(from, portal, to, d);
        }
    }
    fn rect_none(&mut self, from: usize, portal: u16, to: usize) {
        if let Some(t) = self {
            t.rect_none(from, portal, to);
        }
    }
    fn rect_collapse(&mut self, from: usize, portal: u16, to: usize) {
        if let Some(t) = self {
            t.rect_collapse(from, portal, to);
        }
    }
    fn entered(&mut self, from: usize, portal: u16, to: usize, rect: Rect, on_plane: bool) {
        if let Some(t) = self {
            t.entered(from, portal, to, rect, on_plane);
        }
    }
    fn pass2(&mut self, group: usize, flags: u32, window: usize, admitted: bool) {
        if let Some(t) = self {
            t.pass2(group, flags, window, admitted);
        }
    }
    fn pass3(&mut self, group: usize, flags: u32, admitted: bool) {
        if let Some(t) = self {
            t.pass3(group, flags, admitted);
        }
    }
    fn worklist_over_cap(&mut self, windows: usize) {
        if let Some(t) = self {
            t.worklist_over_cap(windows);
        }
    }
}

/// The portal flood. Returns the per-group visible set (indexed by absolute group index). `eye_local` is
/// the camera in WMO model space (WoW axes): it seeds the flood (the group whose floor it stands over —
/// the down-ray) and drives the per-portal front-side test. `clip_from_world` / `world_from_local`
/// project a portal vertex (WoW model space) to clip space via `clip_from_world * world_from_local *
/// wow_to_bevy(v)`. (The per-frame pass now calls [`compute_pvs_traced`] directly with a
/// [`SeedTap`] — the interior-fog resolve reads the seed — so this trace-less wrapper serves the
/// audit harness.)
#[cfg(test)]
fn compute_pvs(
    model: &WmoModel,
    eye_local: [f32; 3],
    terrain_z: Option<f32>,
    clip_from_world: &Mat4,
    world_from_local: &Affine3A,
) -> Vec<bool> {
    compute_pvs_traced(
        model,
        eye_local,
        terrain_z,
        clip_from_world,
        world_from_local,
        &mut (),
    )
    .visible
}

/// What one flood publishes per group: this frame's PVS bit, and whether the group draws with the
/// building's **interior fog** triple rather than the scene fog.
///
/// The second is the client's per-group `[0xca7f00]` — the gate on BOTH interior-fog pushes, the
/// group drawer `0x6b5190` and the group-doodad drawer `0x6b62e0`. Decision 0347 modelled the LANE
/// (interior triple on shared-light rows 18-19, selected by the batch's own `flags & 0x48`
/// interior bit) and recorded this gate as *"not modelled"*; B335 is that gap seen from the
/// Shadowfang courtyard, where the room two exterior-lit courtyards away
/// wore the building's MFOG teal at 70 yd while the reference showed it under the scene fog.
///
/// The rule here: the seed's own group takes the lane (`0x6b3b4c`/`0x6b3df5` force it), and the walk
/// carries it to a neighbour only while no group on the path has `flags & 0x48` — the interior-chain
/// anchor breaking at an exterior/exterior-lit group (`0x6b424f` → `0x6b42c6`, QL-1). A `0x48` group
/// is drawn by the exterior drawer `0x6b4f10`, which pushes no fog at all, so its own bit is inert;
/// what the break decides is the rooms BEYOND it.
pub(crate) struct GroupPvs {
    /// Per absolute group index: in this frame's portal PVS.
    pub(crate) visible: Vec<bool>,
    /// Per absolute group index: draws with the interior fog triple (the `[0xca7f00]` equivalent).
    pub(crate) interior_fog: Vec<bool>,
    /// The **deferred exterior-window worklist** this flood left behind — the client's
    /// `[0xcbe324]` records (count `[0xcbe320]`), one clipped NDC [`Rect`] per doorway the flood
    /// reached onto an `0x8` group. Two consumers, and that is the point of publishing it here:
    /// the flood's own Pass 2 (which exterior groups of THIS building draw) and
    /// [`ExteriorWindows`] (what of the open world draws), which the reference likewise feeds from
    /// one list — `0x681070` `rep movsd`s the very same records into `0xc7cb7c`.
    pub(crate) windows: Vec<Rect>,
    /// How many stack steps every run of the walk took this frame — Pass 1 plus one Pass-2 replay
    /// per window. Instrument only (the dump prints it): Pass 2 re-floods from every window-admitted
    /// exterior group, so this is the number to watch if a dense root ever gets expensive, and the
    /// number that would pin it against [`MAX_ITERS`] if a portal cycle ever ran away.
    pub(crate) iters: u32,
}

/// One step on the portal walk's stack: the group to enter, the group it was entered FROM (so the
/// entry portal is never re-crossed), the working screen rect, the recursion depth, and whether the
/// path that got here kept an unbroken interior chain from the seed.
type Step = (usize, usize, Rect, u32, bool);

/// The portal walk `0x6b41c0`, and the per-FRAME state every run of it shares.
///
/// The inside leg runs this walk **twice**: once from the camera's own group (Pass 1, `0x6b3c05
/// call 0x6b41c0`) and again from every exterior group a deferred window admits (Pass 2, `0x6b3d39
/// call 0x6b41c0` — **the same recursion**, not a draw call). Holding the walk in one place is
/// what makes the second run actually be the same walk: the visible set, the fog lane, the
/// per-portal window stamp and the runaway
/// backstop are all per-frame, not per-pass.
struct Flood<'a> {
    model: &'a WmoModel,
    eye_local: [f32; 3],
    clip_from_world: &'a Mat4,
    world_from_local: &'a Affine3A,
    visible: Vec<bool>,
    interior_fog: Vec<bool>,
    /// The **deferred exterior-window worklist** Pass 1 leaves behind — the client's `[0xcbe324]`
    /// records, reset to empty at `0x6b3b51` and filled by the recursion at `0x6b4541`.
    windows: Vec<Rect>,
    /// The client's per-frame stamp on the PORTAL record (`0x6b4505`, `[esi+0x18]` vs `[0xca8014]`):
    /// a portal contributes **at most one** window per frame, however many paths the flood reaches it
    /// by. It guards the store alone — the recursion runs regardless — so this is a dedup, never a
    /// skip. First push wins, which our stack walk resolves the same way the reference's does.
    portal_pushed: Vec<bool>,
    /// Runaway backstop, counted across every run of the walk in this frame (see [`MAX_ITERS`]).
    iters: u32,
}

impl Flood<'_> {
    /// Drain `stack`, entering each step's group and crossing its portals.
    ///
    /// `record_windows` is the client's inside-leg flag `[0xcde5ac]` — set for Pass 1 only
    /// (`0x6b3b47`, cleared `0x6b3c1a`), which is why Pass 1 is the ONLY run that records deferred
    /// windows: the push itself is guarded on it (`0x6b4511`).
    fn walk<T: FloodTrace>(&mut self, stack: &mut Vec<Step>, record_windows: bool, trace: &mut T) {
        let model = self.model;
        let nav = &model.group_nav;
        let eye_local = self.eye_local;
        while let Some((g, came, rect, depth, chain)) = stack.pop() {
            self.iters += 1;
            if self.iters > MAX_ITERS || g >= nav.len() {
                continue;
            }
            self.visible[g] = true;
            // The interior-fog gate ([`GroupPvs::interior_fog`]): this group draws with the
            // building's own MFOG triple iff the walk that reached it kept an unbroken interior
            // chain from the seed AND the group is itself a true interior. ORed over paths — a
            // group two walks reach takes the lane if EITHER kept the chain.
            //
            // The second term is the DRAWER ROUTER folded in (`0x6b3fe8` picks interior-vs-exterior
            // on the same `flags & 0x48`): an exterior/exterior-lit group is drawn by `0x6b4f10`,
            // which pushes no fog at all, so `[0xca7f00]` cannot reach its surfaces however the walk
            // got there — and that is true of the SEED's own group too when the camera stands in a
            // courtyard. Folding it here rather than ANDing it in each of the three consuming lanes
            // keeps one definition of "which fog does this room wear".
            self.interior_fog[g] |= chain && nav[g].flags & (EXTERIOR | EXTERIOR_LIT) == 0;
            if depth >= DEPTH_CAP {
                continue;
            }
            let nav_g = &nav[g];
            // The chain breaks BELOW an exterior/exterior-lit group, never at the seed: the seed
            // forces the lane on for its own group (`0x6b3b4c`/`0x6b3df5`), and a `flags & 0x48`
            // group is the anchor the walk's chain breaks at (`0x6b424f` → `0x6b42c6`).
            let chain_on = chain && nav_g.flags & (EXTERIOR | EXTERIOR_LIT) == 0;
            let start = nav_g.ref_start as usize;
            let end = (start + nav_g.ref_count as usize).min(model.portal_refs.len());
            for r in &model.portal_refs[start..end] {
                let neighbour = r.group as usize;
                // Never re-cross the entry portal, and skip the "no group" sentinel.
                if r.group == u16::MAX || neighbour == came {
                    continue;
                }
                let Some(info) = model.portal_infos.get(r.portal as usize) else {
                    continue;
                };
                // Front-side test: the camera must be on the `side`-oriented half-space to enter.
                // The threshold IS zero — skip iff `d' < 0` strictly, enter on exactly-0 (the
                // client's `DAT_007ffd74 = 0.0`) — and it ALWAYS runs: the recorded on-plane "skip
                // the side test" flag is dead code in the client (`0x6b43c9`).
                let mut d = info.plane[0] * eye_local[0]
                    + info.plane[1] * eye_local[1]
                    + info.plane[2] * eye_local[2]
                    + info.plane[3];
                if r.side < 0 {
                    d = -d;
                }
                if d < 0.0 {
                    trace.side_fail(g, r.portal, neighbour, d);
                    continue;
                }
                // The eye standing in this portal's plane (inside its polygon) gets the full-screen
                // rect (the client's `0x6b46f0` special case). Without this, a camera crossing a
                // doorway clips the room ahead to nothing for a frame.
                let on_plane = eye_on_portal(&model.portal_vertices, info, eye_local);
                // Project the portal polygon to a screen rect; intersect with the carried rect.
                // Either collapsing to zero area kills this branch (the cathedral case).
                let prect = if on_plane {
                    FULL_SCREEN
                } else {
                    match portal_screen_rect(
                        model,
                        info,
                        self.clip_from_world,
                        self.world_from_local,
                    ) {
                        Some(p) => p,
                        None => {
                            trace.rect_none(g, r.portal, neighbour);
                            continue;
                        }
                    }
                };
                let Some(inter) = intersect_rect(rect, prect) else {
                    trace.rect_collapse(g, r.portal, neighbour);
                    continue;
                };
                trace.entered(g, r.portal, neighbour, inter, on_plane);
                // Reaching a portal onto an exterior group records a deferred window — the client's
                // push at `0x6b44f8`(test `0x8`)/`0x6b4539`(count)/`0x6b4541`(rect).
                // [`opens_a_window`] is the predicate, shared with nothing else because there IS
                // nothing else: this is the one place the worklist is built.
                if record_windows && opens_a_window(nav, neighbour) {
                    if let Some(stamp) = self.portal_pushed.get_mut(r.portal as usize) {
                        if !*stamp {
                            *stamp = true;
                            self.windows.push(inter);
                        }
                    }
                }
                stack.push((neighbour, g, inter, depth + 1, chain_on));
            }
        }
    }
}

/// [`compute_pvs`] with a [`FloodTrace`] tap on every decision — the same flood the frame runs;
/// the probe dump and the audit harness pass a recorder, the per-frame path passes `()`.
fn compute_pvs_traced<T: FloodTrace>(
    model: &WmoModel,
    eye_local: [f32; 3],
    terrain_z: Option<f32>,
    clip_from_world: &Mat4,
    world_from_local: &Affine3A,
    trace: &mut T,
) -> GroupPvs {
    let nav = &model.group_nav;
    let mut flood = Flood {
        model,
        eye_local,
        clip_from_world,
        world_from_local,
        visible: vec![false; nav.len()],
        interior_fog: vec![false; nav.len()],
        windows: Vec::new(),
        portal_pushed: vec![false; model.portal_infos.len()],
        iters: 0,
    };

    // Seed: the down-ray's set — the camera's current group AND, when the nearest crossing was a
    // portal, the group on its other side. Each is an independent flood ROOT with the full screen
    // (the client appends both to the visible-group set `0xc7cd88` and the inside leg loops `visit()`
    // over the whole set (`0x6b3bd4`–`0x6b3c10`); seeding only the containing group under-floods at
    // exactly the near-portal moment the second root exists to cover). Over no surface
    // (open air) or an exterior one ⇒ the outside leg, seeding each EXTERIOR group full-screen (Bevy
    // then frustum-culls the genuinely off-screen ones by their Aabb).
    let mut stack: Vec<Step> = Vec::new();
    let seeds = down_ray_seeds(model, eye_local, terrain_z);
    trace.seed(seeds);
    // **The inside-leg flag** — the client's `[0xcde5ac]`, set only for Pass 1 of `0x6b3b20`
    // (`0x6b3b47`, cleared `0x6b3c1a`), which is why Pass 1 is the ONLY pass that records deferred
    // windows: the push itself is guarded on it (`0x6b4511`). And `0x6b3b20` runs at all only when
    // the camera has a CONTAINING map object — `[0xc7b748]`, whose writer rejects an `0x8` group
    // outright (`0x6be451`, the same rule the interior claim below takes).
    //
    // So the eye standing on a building's own porch or outer terrace is the driver's OUTSIDE leg
    // (`0x6811ca`): no worklist, no Pass 2, the building drawn like any other placement. Without
    // this guard our flood would seed inside that exterior group anyway, push windows from its
    // portal steps, and then let Pass 2 cull the rest of the shell from a vantage where the
    // reference frustum-tests every group normally.
    let inside_leg = seeds
        .in_group
        .and_then(|gi| nav.get(gi))
        .is_some_and(|n| n.flags & EXTERIOR == 0);
    match seeds.in_group {
        Some(c) => {
            stack.push((c, usize::MAX, FULL_SCREEN, 0, true));
            if let Some(a) = seeds.across {
                stack.push((a, usize::MAX, FULL_SCREEN, 0, true));
            }
        }
        None => {
            // The outside leg — no containing map object, so nothing takes the interior fog lane
            // (the client's selector `0x69de20` bails on `[0xc7b748] == 0` ahead of all of this).
            for (gi, g) in nav.iter().enumerate() {
                if g.flags & EXTERIOR != 0 {
                    stack.push((gi, usize::MAX, FULL_SCREEN, 0, false));
                }
            }
        }
    }
    // **Pass 1 — the interior flood** (`0x6b3bd4..0x6b3c10`), the only run that records windows.
    flood.walk(&mut stack, inside_leg, trace);

    // **Pass 2 — the deferred exterior-window replay** (`0x6b3c73..0x6b3d6f`). Gated on the
    // worklist being non-empty (`0x6b3c87 jbe 0x6b3d6f` — a sealed room draws no exterior shell at
    // all), and then, **per window**, every group is admitted on `0x8 ∧ visible against a frustum
    // clipped to THAT window` (`0x6b3d13` / `0x6b3d22`) — never against the full view.
    //
    // **And an admitted group is FLOODED FROM, not merely marked** (`0x6b3d39 call 0x6b41c0` — the
    // portal recursion, the identical entry point Pass 1 uses; the callback draw `0x6b4160` is Pass
    // 3's and only Pass 3's). Decision 1826 shipped this as a marking pass, which is why a city
    // whose outdoor areas are stitched together by portals — Darnassus is one WMO with 104 groups,
    // 50 of them exterior — lost every shell the doorway's own rect did not directly touch. The
    // window admits the *first* group; the walk carries that window's rect onward through the
    // graph, narrowing at each portal exactly as Pass 1 does.
    //
    // We used to mark EVERY `0x8` group visible the moment any window existed, against the full
    // frustum, with a comment calling it "a benign over-draw". It is not benign: it is the
    // building's own far side drawing through its own walls, and decision 0784 had already flagged
    // this exact line — *"the exemption is per placement, not per group … if a case turns up where
    // it is not, this is the line to revisit"*. This is that case.
    //
    // The sub-frustum is [`crate::exterior_cull::window_frustum`] — the same construction the open
    // world is gated by, and **deliberately not** its narrowness reject. That reject is the
    // entry gate of the scene walk `0x682fa0`, not part of the volume builder `0x682930` (which
    // contains no compare at all); 1826 applied it here too and blanked the containing building's
    // whole shell for every window in the `[0.001, 0.02)` NDC band — above the recursion's own
    // collapse guard, below the scene walk's. So a doorway too narrow to admit a hillside can
    // still admit a wall, which is the reference's own asymmetry.
    if flood.windows.len() > WORKLIST_CAP {
        trace.worklist_over_cap(flood.windows.len());
    }
    // The worklist is fixed once Pass 1 ends (`[0xcde5ac]` is clear from here on), so replaying it
    // by value keeps the borrow honest without copying anything that can still change.
    let worklist = std::mem::take(&mut flood.windows);
    for (wi, rect) in worklist.iter().enumerate() {
        let frustum = crate::exterior_cull::window_frustum(*rect, clip_from_world);
        // Every MOGI group is a candidate against every window — the reference's inner loop carries
        // no "already drawn" short-circuit, and one that skipped a visible group would also skip the
        // walk THROUGH it, which is the half of Pass 2 that reaches the rest of the city.
        let mut roots: Vec<Step> = Vec::new();
        for (gi, g) in nav.iter().enumerate() {
            // `¬0x10000 ∧ 0x8` — the client's `0x6b3d0b`/`0x6b3d13`, in that order. A callback-pass
            // group is Pass 3's alone (below); admitting it here too would be the double-submit the
            // reference's two predicates exist to prevent.
            if g.flags & (EXTERIOR | CALLBACK_PASS) != EXTERIOR {
                continue;
            }
            let admitted = frustum.intersects_obb(&group_aabb(g), world_from_local, true, true);
            trace.pass2(gi, g.flags, wi, admitted);
            if admitted {
                roots.push((gi, usize::MAX, *rect, 0, false));
            }
        }
        flood.walk(&mut roots, false, trace);
    }
    flood.windows = worklist;

    // **Pass 3 — the callback pass** (`0x6b3d6f..0x6b3dbc`). Two things make it unlike Pass 2, and
    // both matter: it tests the group against the **full camera frustum** (no window narrowing —
    // there is no `0x682930` call on this path), and it is the `jbe` **fall-through**, so it runs
    // even when the worklist is empty. A `0x10000` group therefore draws from a SEALED interior
    // room, where nothing else of the building's outside does. Its draw is `0x6b4160` → `0x685d70`,
    // the same per-group draw callback the flood itself uses, minus the portal recursion.
    //
    // **Both legs, unlike the two passes above.** Those live in `0x6b3b20`, which runs only when the
    // camera has a containing map object; but the camera-OUTSIDE seed `0x6b3dd0` walks the same MOGI
    // array with the same fork — `flags & 0x10000 -> 0x6b4160`, `flags & 0x8` frustum-pass -> the
    // portal flood — so a callback group is drawn from outdoors by the identical callback. Gating
    // this on the inside leg would have left 20 of the 24 undrawable from anywhere, including
    // `stormwind.wmo#086`, a 322×236×258 yd piece of the city that only `0x6b3dd0` reaches.
    //
    // What this restores: the 24 groups no pass of ours reached from inside — 14 in `stormwind.wmo`
    // (the canals and the tall district shell), 6 across the two Dire Maul roots, and Razorfen
    // Downs'/Kraul's four 450–520 yd enclosing shells. Those last four also carry `0x8`, so before
    // this they were window-tested by Pass 2 and vanished whenever no doorway showed them; the
    // reference draws them regardless.
    // **All four dispatch arms across both legs frustum-test the MOGI AABB first** — inside Pass 3 at
    // `0x6b3d9d`, the outside `0x10000` arm at `0x6b3f2d`, and the two `0x8` arms — with byte-identical
    // setup (`lea ecx,[eax+4]` = the record's six AABB floats, `call 0x682f60`, `test eax,eax; jne`,
    // culled = nonzero), differing only in stack slots. `0x6b4160` itself is `__thiscall(root,
    // groupIndex)`, `ret 4`: no rect, depth, owner or per-group stamp that could make the legs
    // diverge.
    let base = Frustum::from_clip_from_world(clip_from_world);
    for (gi, g) in nav.iter().enumerate() {
        if flood.visible[gi] || g.flags & CALLBACK_PASS == 0 {
            continue;
        }
        let aabb = group_aabb(g);
        let admitted = base.intersects_obb(&aabb, world_from_local, true, true);
        trace.pass3(gi, g.flags, admitted);
        flood.visible[gi] |= admitted;
    }
    GroupPvs {
        visible: flood.visible,
        interior_fog: flood.interior_fog,
        windows: flood.windows,
        iters: flood.iters,
    }
}

/// A group's MOGI bounding box as a Bevy-space [`Aabb`] in the placement's LOCAL frame — the
/// candidate bound Pass 2 hands the window frustum (the client's `0x682f60` takes the same per-group
/// box out of the root's group-info array).
///
/// [`benilla_assets::coords::wow_to_bevy`] is a signed axis permutation, so an axis-aligned box maps
/// to an axis-aligned box exactly: transforming the two corners and taking the componentwise
/// min/max is the whole conversion, with no hull to rebuild.
fn group_aabb(nav: &WmoGroupNav) -> Aabb {
    let a = wow_to_bevy(nav.bbox_min);
    let b = wow_to_bevy(nav.bbox_max);
    Aabb::from_min_max(a.min(b), a.max(b))
}

/// The portal's polygon vertices (WoW model space), or `None` if the span is out of range or
/// degenerate (< 3 vertices).
fn portal_poly<'a>(
    portal_vertices: &'a [[f32; 3]],
    info: &WmoPortalInfo,
) -> Option<&'a [[f32; 3]]> {
    let start = info.start_vertex as usize;
    let verts = portal_vertices.get(start..start + info.count as usize)?;
    (verts.len() >= 3).then_some(verts)
}

/// Does the eye lie **in** this portal — within [`ON_PLANE_EPS`] of its plane (inclusive, the client's
/// `|d| <= 0.01`) and inside its polygon (dominant-axis 2-D projection, the client's `0x7c23e0`)? The
/// flood then gives the portal the full-screen rect — the client's `0x6b46f0` special case. The side
/// test still applies (`0x6b43c9`: the skip flag is dead).
fn eye_on_portal(portal_vertices: &[[f32; 3]], info: &WmoPortalInfo, eye: [f32; 3]) -> bool {
    let [nx, ny, nz, d] = info.plane;
    if (nx * eye[0] + ny * eye[1] + nz * eye[2] + d).abs() > ON_PLANE_EPS {
        return false;
    }
    let Some(verts) = portal_poly(portal_vertices, info) else {
        return false;
    };
    // Project out the plane's dominant axis; the eye is on the plane, so its projection is itself.
    let (u, v) = dominant_axes(info.plane);
    point_in_poly_2d(verts.iter().map(|p| (p[u], p[v])), (eye[u], eye[v]))
}

/// Even-odd point-in-polygon on 2-D projected vertices.
fn point_in_poly_2d(pts: impl Iterator<Item = (f32, f32)> + Clone, p: (f32, f32)) -> bool {
    let mut inside = false;
    let mut prev = pts.clone().last();
    for cur in pts {
        if let Some(pr) = prev {
            if (cur.1 > p.1) != (pr.1 > p.1)
                && p.0 < (pr.0 - cur.0) * (p.1 - cur.1) / (pr.1 - cur.1) + cur.0
            {
                inside = !inside;
            }
        }
        prev = Some(cur);
    }
    inside
}

/// Project a portal polygon to its NDC bounding rectangle — the client's `0x6b46f0`: transform to
/// clip space, Sutherland–Hodgman clip against the four **side** planes of the view pyramid (there is
/// NO near-plane clip), then min/max the per-vertex NDC with the faithful `w` handling: `|w| < 0.001`
/// substitutes `w = +1e-5` (positive regardless of the vertex's sign); a vertex still carrying
/// `w <= -0.001` divides by its real negative `w`, so its **mirrored** NDC enters the rect. That is
/// what keeps a doorway the eye is straddling wide open (the boundary points at the eye clamp to
/// `+1e-5` and blow the rect out) instead of collapsing it for a frame. The rect is returned raw
/// (un-clamped) — the caller's intersect with the carried rect bounds it. `None` iff the polygon is
/// fully clipped (degenerate / off-screen) — the client's `rc.flags |= 0x1` skip.
fn portal_screen_rect(
    model: &WmoModel,
    info: &WmoPortalInfo,
    clip_from_world: &Mat4,
    world_from_local: &Affine3A,
) -> Option<Rect> {
    let verts = portal_poly(&model.portal_vertices, info)?;
    // WoW model vertex → world → clip (the same path the rendered geometry takes).
    let clip: Vec<Vec4> = verts
        .iter()
        .map(|v| {
            let world = world_from_local.transform_point3(wow_to_bevy(*v));
            *clip_from_world * world.extend(1.0)
        })
        .collect();
    let clipped = clip_side_planes(&clip)?;
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in &clipped {
        let w = if c.w.abs() < W_CLAMP_BAND {
            W_CLAMP_SUB
        } else {
            c.w
        };
        let inv = 1.0 / w;
        let (x, y) = (c.x * inv, c.y * inv);
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    Some([min_x, min_y, max_x, max_y])
}

/// Sutherland–Hodgman clip of a clip-space polygon against the four **side** planes of the view
/// pyramid (`x ≥ −w`, `x ≤ w`, `y ≥ −w`, `y ≤ w`, each linear in clip space — the client's guard
/// pyramid `0xc7d0dc` fed to `0x6b4a80`). Deliberately no near plane: a polygon spanning
/// the eye survives as boundary points at/near `w = 0`, which the caller's `w`-clamp handles — the
/// faithful behaviour. `None` if fewer than 3 vertices remain (fully clipped).
fn clip_side_planes(poly: &[Vec4]) -> Option<Vec<Vec4>> {
    const PLANES: [fn(&Vec4) -> f32; 4] =
        [|v| v.w + v.x, |v| v.w - v.x, |v| v.w + v.y, |v| v.w - v.y];
    let mut cur = poly.to_vec();
    for f in PLANES {
        let n = cur.len();
        let mut out: Vec<Vec4> = Vec::with_capacity(n + 2);
        for i in 0..n {
            let a = cur[i];
            let b = cur[(i + 1) % n];
            let (fa, fb) = (f(&a), f(&b));
            if fa >= 0.0 {
                out.push(a);
            }
            if (fa >= 0.0) != (fb >= 0.0) {
                let t = fa / (fa - fb);
                out.push(a + (b - a) * t);
            }
        }
        cur = out;
        if cur.len() < 3 {
            return None;
        }
    }
    Some(cur)
}

/// Intersect two NDC rects; `None` if they don't overlap with non-zero area.
fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let rect = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (rect[2] - rect[0] >= RECT_EPS && rect[3] - rect[1] >= RECT_EPS).then_some(rect)
}

#[cfg(test)]
mod audit;

#[cfg(test)]
mod tests {
    use super::*;

    use benilla_assets::WmoPortalRef;

    /// A prop is drawn while ANY room that names it is visible — the reference's per-visible-group
    /// MODR walk. Blackrock's lava falls hang through up to 19 groups; keying them to one owner hid
    /// the fall from every camera angle whose flood reached a different referrer. Geometry (a
    /// single-group key) is unaffected, and an index past the visible set fails OPEN.
    #[test]
    fn a_prop_is_drawn_while_any_of_its_rooms_is_visible() {
        let inst = WmoPortalInstance {
            handle: Handle::default(),
            world_from_local: Affine3A::IDENTITY,
            name_set: 0,
            visible: vec![false, true, false],
            interior_fog: vec![false; 3],
            liquid_visited: vec![false; 3],
            flooded: vec![None; 3],
        };
        let key = |groups: &[u16]| WmoGroupVis {
            instance: Entity::PLACEHOLDER,
            groups: Arc::from(groups),
        };
        assert!(key(&[1]).drawn_by(&inst), "its own room is visible");
        assert!(!key(&[0]).drawn_by(&inst), "its own room is culled");
        assert!(
            key(&[0, 2, 1]).drawn_by(&inst),
            "one visible referrer out of three draws the fall"
        );
        assert!(!key(&[0, 2]).drawn_by(&inst), "no referrer is visible");
        assert!(
            !key(&[]).drawn_by(&inst),
            "no referrers at all: nothing ORs"
        );
        assert!(key(&[9]).drawn_by(&inst), "index past the set fails open");
    }

    /// The interior-fog key ORs like `drawn_by` — a prop several rooms name wears the fog of any
    /// of them — but a lookup miss reads FALSE where `drawn_by`'s reads true. The asymmetry is
    /// deliberate: a missing PVS bit must not blank a building, and a missing fog bit must not
    /// paint a room's MFOG onto something the flood never placed.
    #[test]
    fn the_interior_fog_key_ors_over_rooms_and_fails_closed() {
        let inst = WmoPortalInstance {
            handle: Handle::default(),
            world_from_local: Affine3A::IDENTITY,
            name_set: 0,
            visible: vec![true; 3],
            interior_fog: vec![false, true, false],
            liquid_visited: vec![false; 3],
            flooded: vec![None; 3],
        };
        let key = |groups: &[u16]| WmoGroupVis {
            instance: Entity::PLACEHOLDER,
            groups: Arc::from(groups),
        };
        assert!(key(&[1]).interior_fogged_by(&inst), "its own room is on it");
        assert!(
            !key(&[0]).interior_fogged_by(&inst),
            "its own room is off it"
        );
        assert!(
            key(&[0, 2, 1]).interior_fogged_by(&inst),
            "one referrer on the lane is enough"
        );
        assert!(
            !key(&[0, 2]).interior_fogged_by(&inst),
            "no referrer is on it"
        );
        assert!(
            !key(&[]).interior_fogged_by(&inst),
            "no referrers: nothing ORs"
        );
        assert!(
            !key(&[9]).interior_fogged_by(&inst),
            "index past the set fails CLOSED — the opposite of drawn_by"
        );
    }

    /// **Only `0x8` opens a window onto the open world** (`0x6b44f8`), and the flag values here are
    /// Stormwind's own, read off a live cull trace at the director's `.go xyz -8798.70 478.64 95.79`
    /// (decision 1148). The city is 306 groups of which **exactly one** — `g46`, `0x08809` — carries
    /// `0x8`; the streets and rooms are `0x40`-without-`0x8` (decision 0475). Testing the weather
    /// global's `0x148` instead turned all eleven rooms the flood reached from that spot into
    /// doorways onto Elwynn, one of them 74% of the screen wide. If this test ever goes green on
    /// `0x148` again, the whole exterior cull is off in every city.
    #[test]
    fn only_an_exterior_group_opens_a_window_onto_the_open_world() {
        let g = |flags: u32| WmoGroupNav {
            flags,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            ref_start: 0,
            ref_count: 0,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: 0xf,
        };
        // Stormwind's real group flags: the rooms/streets the flood reached from the reported spot…
        let nav = [g(0x0b841), g(0x0a841), g(0x0ba41), g(0x08809), g(0x02a05)];
        for to in [0, 1, 2] {
            assert!(
                !opens_a_window(&nav, to),
                "g{to} is EXTERIOR_LIT (0x40) without EXTERIOR (0x8) — a lit indoor room, not a \
                 doorway onto Elwynn"
            );
        }
        assert!(!opens_a_window(&nav, 4), "0x02a05 carries neither bit");
        // …and g46, the city's single exterior shell, which is the only real window.
        assert!(
            opens_a_window(&nav, 3),
            "0x08809 carries 0x8 — this one IS the open world"
        );
        // A destination past the nav (a corrupt MOPR ref) fails CLOSED: an unknown group is not
        // evidence of a doorway, and failing open here is the bug this test exists to catch.
        assert!(
            !opens_a_window(&nav, 99),
            "an out-of-range group is no window"
        );
    }

    #[test]
    fn eye_on_portal_only_within_the_band_and_polygon() {
        // A vertical doorway portal in the x=0 plane, spanning y ∈ [-2,2], z ∈ [0,4].
        let verts = vec![
            [0.0, -2.0, 0.0],
            [0.0, 2.0, 0.0],
            [0.0, 2.0, 4.0],
            [0.0, -2.0, 4.0],
        ];
        let info = WmoPortalInfo {
            start_vertex: 0,
            count: 4,
            plane: [1.0, 0.0, 0.0, 0.0],
        };
        // In the plane, inside the polygon ⇒ the doorway is wide open.
        assert!(eye_on_portal(&verts, &info, [0.0, 0.0, 2.0]));
        assert!(eye_on_portal(&verts, &info, [0.005, 1.5, 3.5]));
        // In the plane but outside the polygon (above the door) ⇒ not standing in it.
        assert!(!eye_on_portal(&verts, &info, [0.0, 0.0, 5.0]));
        // Inside the polygon's span but off the plane ⇒ the normal side/rect path applies.
        assert!(!eye_on_portal(&verts, &info, [0.5, 0.0, 2.0]));
    }

    #[test]
    fn point_in_poly_2d_hits_inside_and_misses_outside() {
        let sq = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        assert!(point_in_poly_2d(sq.iter().copied(), (0.0, 0.0)));
        assert!(point_in_poly_2d(sq.iter().copied(), (-0.9, 0.9)));
        assert!(!point_in_poly_2d(sq.iter().copied(), (1.5, 0.0)));
        assert!(!point_in_poly_2d(sq.iter().copied(), (0.0, -1.5)));
    }

    #[test]
    fn intersect_rect_collapses_to_none() {
        // Overlapping rects intersect to the overlap.
        assert_eq!(
            intersect_rect([-1.0, -1.0, 0.0, 0.0], [-0.5, -0.5, 1.0, 1.0]),
            Some([-0.5, -0.5, 0.0, 0.0])
        );
        // Disjoint rects ⇒ the window collapses (the cathedral-cull termination).
        assert_eq!(
            intersect_rect([-1.0, -1.0, -0.5, -0.5], [0.5, 0.5, 1.0, 1.0]),
            None
        );
        // A sliver thinner than the client's 0.001 zero-area eps also collapses.
        assert_eq!(
            intersect_rect([-1.0, -1.0, 1.0, 1.0], [0.0, -1.0, 0.0005, 1.0]),
            None
        );
    }

    #[test]
    fn clip_side_planes_drops_behind_and_off_screen_keeps_straddling() {
        // Fully behind the eye (w < 0, inside the mirror cone) ⇒ outside every side half-space ⇒
        // fully clipped, like the client's rc.flags |= 0x1.
        let behind = [
            Vec4::new(0.0, 0.0, 0.0, -1.0),
            Vec4::new(0.1, 0.0, 0.0, -1.0),
            Vec4::new(0.0, 0.1, 0.0, -1.0),
        ];
        assert!(clip_side_planes(&behind).is_none());
        // Fully in front and on-screen ⇒ unchanged triangle.
        let front = [
            Vec4::new(0.0, 0.0, 0.0, 1.0),
            Vec4::new(0.5, 0.0, 0.0, 1.0),
            Vec4::new(0.0, 0.5, 0.0, 1.0),
        ];
        assert_eq!(clip_side_planes(&front).map(|p| p.len()), Some(3));
        // Fully off-screen laterally (x > w for every vertex) ⇒ fully clipped.
        let off = [
            Vec4::new(2.0, 0.0, 0.0, 1.0),
            Vec4::new(3.0, 0.0, 0.0, 1.0),
            Vec4::new(2.0, 1.0, 0.0, 1.0),
        ];
        assert!(clip_side_planes(&off).is_none());
        // Straddling the eye plane (one vertex behind): survives — the boundary points land at/near
        // w = 0 for the w-clamp to handle. THIS is the case a near-plane clip would mishandle.
        let straddle = [
            Vec4::new(0.0, -0.5, 0.0, 1.0),
            Vec4::new(0.0, 0.5, 0.0, 1.0),
            Vec4::new(0.0, 0.0, 0.0, -0.5),
        ];
        assert!(clip_side_planes(&straddle).is_some());
    }

    #[test]
    fn portal_rect_explodes_open_on_the_doorway_approach() {
        // The camera 0.05 yd before the doorway plane (outside the 0.01 on-plane band), looking
        // through it: the polygon fills far past the screen edges, and with no near-plane clip the
        // rect must blow out to cover the whole screen — the client's protection for the approach
        // frames (the crossing frame itself is the on-plane full-screen case).
        let model = portal_model();
        let info = &model.portal_infos[0];
        // WoW (−0.05, 0, 0) → Bevy (0, 0, 0.05); looking along WoW +x <=> Bevy −z.
        let view = Mat4::look_at_rh(
            bevy::math::Vec3::new(0.0, 0.0, 0.05),
            bevy::math::Vec3::new(0.0, 0.0, -10.0),
            bevy::math::Vec3::Y,
        );
        let clip = Mat4::perspective_rh(0.9, 16.0 / 9.0, 0.1, 1000.0) * view;
        let rect = portal_screen_rect(&model, info, &clip, &Affine3A::IDENTITY)
            .expect("a doorway ahead must keep a rect");
        let inter = intersect_rect(FULL_SCREEN, rect).expect("must overlap the screen");
        // The window is the whole screen while the eye is this deep in the doorway.
        assert!(inter[0] <= -0.99 && inter[1] <= -0.99 && inter[2] >= 0.99 && inter[3] >= 0.99);
    }

    #[test]
    fn portal_rect_is_none_when_fully_behind_the_camera() {
        let model = portal_model();
        let info = &model.portal_infos[0];
        // Camera 5 yd past the doorway (WoW +x maps to Bevy −z), still looking further along −z:
        // the doorway is squarely behind ⇒ fully clipped ⇒ no rect.
        let view = Mat4::look_at_rh(
            bevy::math::Vec3::new(0.0, 0.0, -5.0),
            bevy::math::Vec3::new(0.0, 0.0, -10.0),
            bevy::math::Vec3::Y,
        );
        let clip = Mat4::perspective_rh(0.9, 16.0 / 9.0, 0.1, 1000.0) * view;
        assert!(portal_screen_rect(&model, info, &clip, &Affine3A::IDENTITY).is_none());
    }

    /// **Pass 2 is a per-window test, not a blanket** (decision 1826, `0x6b3d13`/`0x6b3d22`). One
    /// interior room with a narrow doorway onto the outdoors, and two portal-disconnected exterior
    /// shells: one straight through the doorway,
    /// one 30 yd off to the side. The reference draws each `0x8` group only where a *deferred
    /// window* admits it, so the shell in the doorway draws and the one beside it does not.
    ///
    /// Before 1826 we set every `0x8` group visible the moment any window existed — so `g3`, a wall
    /// no doorway shows, drew through the room's own walls. If this test ever goes green with `g3`
    /// visible again, the building's far side is leaking again.
    #[test]
    fn pass_two_admits_only_the_exterior_shells_a_doorway_shows() {
        let model = doorway_model();
        // Eye 3 yd back from the doorway, WoW (-3, 0, 1) = Bevy (0, 1, 3), looking along WoW +x
        // (Bevy −z) — straight through the opening.
        let eye_local = [-3.0, 0.0, 1.0];
        let view = Mat4::look_at_rh(
            bevy::math::Vec3::new(0.0, 1.0, 3.0),
            bevy::math::Vec3::new(0.0, 1.0, -10.0),
            bevy::math::Vec3::Y,
        );
        let clip = Mat4::perspective_rh(0.9, 16.0 / 9.0, 0.1, 1000.0) * view;
        let pvs = compute_pvs_traced(&model, eye_local, None, &clip, &Affine3A::IDENTITY, &mut ());

        assert_eq!(
            pvs.windows.len(),
            1,
            "the one portal onto an exterior group is the one deferred window"
        );
        assert!(pvs.visible[0], "the camera's own room");
        assert!(pvs.visible[1], "the exterior group the portal opens onto");
        assert!(
            pvs.visible[2],
            "a portal-disconnected shell straight through the doorway is admitted by its window \
             — this is the half the flood alone can never reach"
        );
        assert!(
            !pvs.visible[3],
            "a portal-disconnected shell 30 yd off the doorway's axis is culled: no window shows \
             it, and drawing it is exactly the over-draw 1826 removed"
        );
        assert!(
            pvs.visible[4],
            "1853: an admitted shell is FLOODED FROM, not merely marked — g4 is an interior behind \
             g2 with no edge to the seed's half of the graph, so the walk through g2's own back \
             door is its only route into the PVS (`0x6b3d39 call 0x6b41c0`). A marking Pass 2 \
             leaves it culled, which from a Darnassus shop is most of the city"
        );

        // The control: widen the window to the whole screen (stand IN the doorway, which takes the
        // on-plane full-screen branch) and the side shell comes back — proving the cull above is the
        // window's doing and not the group's.
        let in_door = compute_pvs_traced(
            &model,
            [0.0, 0.0, 1.0],
            None,
            &clip,
            &Affine3A::IDENTITY,
            &mut (),
        );
        assert!(
            in_door.visible[3],
            "with the doorway full-screen, every shell in front of the camera draws"
        );
    }

    /// **Pass 3 draws a `0x10000` group from a SEALED room** — the `jbe` fall-through at
    /// `0x6b3d6f`, against the full frustum, gated on nothing (decision 1826). This is the half
    /// Pass 2 cannot express: with an empty worklist Pass 2 does not run at all, and every `0x8`
    /// group is correctly dark, yet a callback-pass group still draws.
    ///
    /// The subjects in the real data are Razorfen Downs'/Kraul's 450–520 yd enclosing shells (which
    /// carry `0x8` **and** `0x10000`, so Pass 2 alone would have culled them whenever no doorway
    /// showed them) and Stormwind's canals.
    #[test]
    fn pass_three_draws_a_callback_group_from_a_sealed_room() {
        // The doorway model with its portal graph removed: the room is sealed, so the flood reaches
        // nothing and leaves no windows. g1..g3 keep their `0x8`; g3 also gains `0x10000`.
        let mut model = doorway_model();
        model.portal_refs.clear();
        model.group_nav[0].ref_count = 0;
        model.group_nav[1].ref_count = 0;
        model.group_nav[3].flags |= CALLBACK_PASS;
        // g3 sits 30 yd off-axis; aim the camera at it so the FULL frustum contains it while no
        // window ever could — WoW (2..6, 30..34) → Bevy x ∈ [-34,-30], z ∈ [-6,-2].
        let view = Mat4::look_at_rh(
            bevy::math::Vec3::new(0.0, 1.0, 3.0),
            bevy::math::Vec3::new(-32.0, 1.0, -4.0),
            bevy::math::Vec3::Y,
        );
        let clip = Mat4::perspective_rh(0.9, 16.0 / 9.0, 0.1, 1000.0) * view;
        let pvs = compute_pvs_traced(
            &model,
            [-3.0, 0.0, 1.0],
            None,
            &clip,
            &Affine3A::IDENTITY,
            &mut (),
        );

        assert!(
            pvs.windows.is_empty(),
            "a sealed room opens no doorway onto the outdoors"
        );
        assert!(pvs.visible[0], "the camera's own room");
        assert!(
            !pvs.visible[1] && !pvs.visible[2],
            "plain 0x8 shells stay dark with an empty worklist — Pass 2 never runs"
        );
        assert!(
            pvs.visible[3],
            "the 0x10000 group draws anyway: Pass 3 is the jbe fall-through, full frustum, ungated"
        );

        // The control: Pass 3 is a frustum test, not an unconditional pass. Turn away from g3 and it
        // goes dark — otherwise this test would pass just as well on a blanket.
        let away = Mat4::perspective_rh(0.9, 16.0 / 9.0, 0.1, 1000.0)
            * Mat4::look_at_rh(
                bevy::math::Vec3::new(0.0, 1.0, 3.0),
                bevy::math::Vec3::new(32.0, 1.0, 10.0),
                bevy::math::Vec3::Y,
            );
        let behind = compute_pvs_traced(
            &model,
            [-3.0, 0.0, 1.0],
            None,
            &away,
            &Affine3A::IDENTITY,
            &mut (),
        );
        assert!(
            !behind.visible[3],
            "a callback group behind the camera is still frustum-culled"
        );

        // …and the OUTSIDE leg draws it too — the camera-outside seed `0x6b3dd0` forks on the same
        // `0x10000` before its `0x8` frustum-pass. Eye over open ground (no floor under it ⇒ no
        // containing group), so the flood takes its outside branch and seeds only `0x8` groups;
        // g3 has no portal refs and would be unreachable, yet the callback arm still draws it.
        let outside = compute_pvs_traced(
            &model,
            [-3.0, 0.0, 500.0],
            None,
            &clip,
            &Affine3A::IDENTITY,
            &mut (),
        );
        assert!(
            outside.visible[3],
            "the callback arm belongs to BOTH legs — gating it on the inside leg left 20 of the 24 \
             real groups undrawable from anywhere"
        );
    }

    /// One interior room (`g0`) whose single doorway (`p0`, WoW x=0, y ∈ [-0.5,0.5], z ∈ [0,2]) opens
    /// onto an exterior group (`g1`), plus two portal-DISCONNECTED exterior shells: `g2` straight
    /// ahead through the opening, `g3` 30 yd to the side. `g0` carries a floor at z=0 so the down-ray
    /// seeds the flood there.
    fn doorway_model() -> WmoModel {
        let nav = |flags: u32, min: [f32; 3], max: [f32; 3], ref_start: u16, ref_count: u16| {
            WmoGroupNav {
                flags,
                bbox_min: min,
                bbox_max: max,
                ref_start,
                ref_count,
                area_table_id: 0,
                fog_indices: [0; 4],
                group_liquid: benilla_formats::NO_GROUP_LIQUID,
            }
        };
        let floor = |z: f32| {
            vec![
                [[-10.0, -10.0, z], [0.0, -10.0, z], [0.0, 10.0, z]],
                [[-10.0, -10.0, z], [0.0, 10.0, z], [-10.0, 10.0, z]],
            ]
        };
        WmoModel {
            portal_vertices: vec![
                [0.0, -0.5, 0.0],
                [0.0, 0.5, 0.0],
                [0.0, 0.5, 2.0],
                [0.0, -0.5, 2.0],
                // p1, the back door of the disconnected shell g2 — g4's only way in.
                [6.0, -0.5, 0.0],
                [6.0, 0.5, 0.0],
                [6.0, 0.5, 2.0],
                [6.0, -0.5, 2.0],
            ],
            portal_infos: vec![
                WmoPortalInfo {
                    start_vertex: 0,
                    count: 4,
                    plane: [1.0, 0.0, 0.0, 0.0],
                },
                WmoPortalInfo {
                    start_vertex: 4,
                    count: 4,
                    plane: [1.0, 0.0, 0.0, -6.0],
                },
            ],
            // g0 -> g1 and back. `side = -1` puts the eye at x < 0 on the entering half-space.
            // Then g2 -> g4 and back, through p1: g2 has no edge to the seed's side of the graph, so
            // the ONLY route to g4 is Pass 2 admitting g2 and then walking on through it.
            portal_refs: vec![
                WmoPortalRef {
                    portal: 0,
                    group: 1,
                    side: -1,
                },
                WmoPortalRef {
                    portal: 0,
                    group: 0,
                    side: 1,
                },
                WmoPortalRef {
                    portal: 1,
                    group: 4,
                    side: -1,
                },
                WmoPortalRef {
                    portal: 1,
                    group: 2,
                    side: 1,
                },
            ],
            group_nav: vec![
                nav(0x2000, [-10.0, -10.0, 0.0], [0.0, 10.0, 3.0], 0, 1),
                nav(EXTERIOR, [1.0, -0.5, 0.0], [5.0, 0.5, 2.0], 1, 1),
                nav(EXTERIOR, [2.0, -0.5, 0.0], [6.0, 0.5, 2.0], 2, 1),
                nav(EXTERIOR, [2.0, 30.0, 0.0], [6.0, 34.0, 2.0], 0, 0),
                nav(0x2000, [7.0, -0.5, 0.0], [11.0, 0.5, 2.0], 3, 1),
            ],
            group_collision_tris: vec![floor(0.0), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            ..portal_model()
        }
    }

    /// A bare model holding one vertical doorway portal in the WoW x=0 plane (y ∈ [-1,1], z ∈ [-1,1]),
    /// i.e. straddling the world origin.
    fn portal_model() -> WmoModel {
        WmoModel {
            wmo_id: 0,
            submeshes: Vec::new(),
            submesh_group: Vec::new(),
            portal_vertices: vec![
                [0.0, -1.0, -1.0],
                [0.0, 1.0, -1.0],
                [0.0, 1.0, 1.0],
                [0.0, -1.0, 1.0],
            ],
            portal_infos: vec![WmoPortalInfo {
                start_vertex: 0,
                count: 4,
                plane: [1.0, 0.0, 0.0, 0.0],
            }],
            portal_refs: Vec::new(),
            group_nav: Vec::new(),
            fogs: Vec::new(),
            skybox: None,
            group_collision_tris: Vec::new(),
            group_camera_only_tris: Vec::new(),
            group_collision_bounds: Vec::new(),
            group_collision_grids: Vec::new(),
            collision_bounds: None,
            collision: None,
            collision_camera: None,
            doodads: Vec::new(),
            doodad_sets: Vec::new(),
            lights: Vec::new(),
            group_bounds: Vec::new(),
            group_footprints: Vec::new(),
            material_ground_type: Vec::new(),
            material_diff_color: Vec::new(),
            group_footprint_bounds: Vec::new(),
            group_footprint_grids: Vec::new(),
            group_light_refs: Vec::new(),
            group_liquids: Vec::new(),
            doodad_base: Default::default(),
            doodad_owner: Default::default(),
            doodad_groups: Default::default(),
        }
    }
}
