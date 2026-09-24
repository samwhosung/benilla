//! The **skybox** — an authored sky model that stands in for the `Light.dbc` gradient dome, drawn
//! camera-anchored in the sky slice.
//!
//! **Two sources feed one lane, and the DBC source wins.** A *building* asks for its own painted sky
//! (the **MOSB**/`0x40000` mechanism derived below); the *death profile* asks for the ghost sky
//! (`LightSkybox.dbc` via `LightParams.lightSkyboxID`, [`benilla_formats::LightCatalog::ghost_skybox`]).
//! The reference keeps these in separate slots — the WMO's is loop slot A, the DBC's is the single
//! slot B — and slot B, whose weight is hardcoded `1.0` whenever it is filled, makes
//! `0x6d4ac1`–`0x6d4acc` skip loop A entirely: **an active DBC skybox suppresses the WMO one
//! outright**. Here that is one
//! `Option` with the DBC source taken first, because a resolve that produced both would have to
//! decide anyway and the binary has already decided.
//!
//! A WMO root can name a skybox model in its **MOSB** chunk, and a group can ask for it with group
//! flag **`0x40000`**. Both halves matter, and the flag is tested **on the groups the portal flood
//! REACHES, never on the group the camera stands in** — `0x6b42e0` sits inside the flood `0x6b41c0`
//! with `ebx` = the group being *visited*, seeded from the containment list on the inside leg and
//! from every frustum-visible EXTERIOR group on the outside leg (`0x6b3dd0`), recursing through
//! portals at `0x6b4639` and re-testing at each step. The predicate is therefore
//! *"any flood-reached group carries the bit, and the root names a MOSB"*, published to `[0xca8080]`
//! and read once at `0x681282`.
//!
//! **That distinction is load-bearing, and getting it wrong is what shipped first.** In Stratholme's
//! King's Square the camera stands in group 39 — the root's *only* EXTERIOR (`0x8`) group, which does
//! **not** set `0x40000`. A containing-group test draws no skybox there; the reference draws the
//! painted red sky, because 61 of the 83 groups its flood reaches from group 39 do carry the bit.
//! (Checked on the asset: a BFS over `Stratholme_B`'s MOPR reaches 82 of 83 groups from group 39.)
//! The corpus correlation that seeded the first attempt — across all 815 roots the bit never appears
//! without a MOSB, 0 counter-examples (`benilla-extract skyboxscan`) — was true and still is; it just
//! never established *which* group the renderer tests, and it was over-read as if it had.
//!
//! Five roots exercise both halves: **`Stratholme_B`**, the burning city, and the four Caverns of
//! Time shells — which are *not* out of reach in 1.12, whatever the instance portals do.
//! `CavernsofTime.wmo` is placed in the live world at Kalimdor tile (39, 47), MODF `uniqueId`
//! 398759, and standing in the crater at `.go xyz -8437.16 -4222.44 -211.58 1` the cull's down-ray
//! seeds group 29 (`flags 0x42805` — `SHOW_SKYBOX` set) and this resolve fires. An earlier note here
//! read "unreleased" as "unreachable" and concluded this branch never actually picks; the director
//! walked into it.
//!
//! Four more roots name a skybox no group ever asks for (DireMaul's
//! instance shell, `Stratholme_A`, and the two Sunken Temple roots — whose MOSB isn't even a model
//! path, it's the string "the temple of atal'hakkar"), so keying off the chunk alone would still
//! paint skies the reference never shows. This is why Stratholme's sky is red where the zone light
//! says otherwise: map 329's only reachable `Light.dbc` atmosphere (global row 341 → `LightParams`
//! 336) is a khaki-brown gradient with a near-black apex, and it is not what draws in there.
//!
//! **A skybox is an ordinary M2, and is drawn as one** (decision 1264). This lane used to have a
//! private mesh builder and a private material — positions, UVs, one texture, everything opaque —
//! which is a faithful drawing of `StratholmeSkybox.m2` (three opaque batches × 8 verts, one texture
//! pair per axis, no animation) and of nothing else. `CavernsOfTimeSky.m2` is the counter-example the
//! chain also ships: **21 batches across four blend modes** — a painted cube, six ADDITIVE star
//! sheets on the cube's own faces, five alpha-blended planet cards, three alpha-tested asteroid belts
//! on rotating bones. Drawn opaque, the star sheet — whose RGB is near-white and whose stars live in
//! its ALPHA channel — paints a flat white sheet over the painted sky, and the planets and belts
//! become dark cards. That is the director's *"the whole ceiling is white … some of the cool effects
//! seem missing"* in Caverns of Time.
//!
//! So the batches go through [`crate::model_render`]'s material lane like every other model
//! ([`M2BatchMaterials::skybox`]), which is where the blend law, the 224/255 alpha-key reference, the
//! additive gamma premultiply and the authored batch order already live, byte-verified. Three things
//! are the *sky's* and are set here, not read off the batch: the forced far depth, depth-write off,
//! and depth-test on — the rationale is on `skybox()`.
//!
//! Occlusion is **not** the box's radius: like every sky element it forces the far depth (the law is
//! in [`crate::sky_order`]), so the world paints over it and the shell is free to sit inside the
//! room's own geometry.
//!
//! **The slot has a WEIGHT, and the weight is the 4-second crossfade.** The WMO slot is filled per
//! frame as `0x6d4810(0, [0xca8080], [0xce9bdc])` — and `[0xce9bdc]` is the camera-in-WMO interior
//! crossfade, THE same number the MFOG fog lerp rides (±0.25/s = `[0x8115b0]`, clamped [0,1]) —
//! the skybox alpha and the interior fog blend are the same number.
//! A slot draws only at weight > 0 (`0x6d4afe`), with the weight multiplied into every batch's
//! combined alpha (`0x710cb0` → `[CM2Model+0x180]`) — and a batch at `0 < A < 1` is promoted to
//! SRC_ALPHA blending whatever its authored mode (`0x811fe0`), so walking
//! through Stratholme's gate alpha-blends the painted sky in over the still-standing celestial
//! pass over 4 seconds, and back out on leaving. Here: [`crate::lighting::WmoCrossfade`] publishes
//! the number, [`SkyboxWeight`] resolves the slot's weight from it (the DBC/ghost slot is
//! hardcoded 1.0 — that pop is faithful), and [`apply_skybox_visibility`] applies it through the
//! entity feather rails (`MeshTag` alpha + the promotion twin).
//!
//! **Scope — the skybox replaces the WHOLE celestial pass, not just the backdrop.** `CSky::Render`
//! carries one shared boolean (`0x6d49cd` sets it, `0x6d49fb`/`0x6d4a2e` clear it once any slot's
//! weight exceeds `[0x808aac]` = 0.99) and `0x6d4a3b test edi,edi; je` skips **all six** element
//! draws together — stars, sun disc, both moons, gradient band and cloud dome. There is no
//! per-element gating; below the 0.99 threshold — i.e. for all but the last 40 ms of the fade —
//! the six elements still draw and the skybox blends over them ([`SkyboxWeight::replaces_celestial`]).
//! Confirmed in a live GL capture of the reference standing in King's Square:
//! the entire `[0.975, 0.98]` sky slice is three `count=12` draws (the cube's three texture pairs)
//! and nothing else, identically in every frame. Only the **glare** quads survive — they render on
//! their own later path (`0x483740 → 0x6d48c0 → 0x7e57e0`), outside this pass.
//!
//! What the atmosphere drives (fog colour and distance, ambient, diffuse) is still untouched: no byte
//! law says the flag reaches it, and the reference fogs the world normally underneath the painted sky.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;

use crate::model_render::M2BatchMaterials;
use crate::view::WorldCamera;
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::WmoModel;
use benilla_assets::{LockRecover, WorldAssets};

/// MOGP/MOGI group flag **SHOW_SKYBOX** — this group draws its root's MOSB model as the sky (see the
/// module doc for how the bit was identified). Mirrored between the root's MOGI table and each group
/// file's MOGP header; we read the MOGP copy the loader already keeps in `WmoGroupNav::flags`.
const SHOW_SKYBOX: u32 = 0x40000;

/// The skybox model the camera's room asks for this frame — `None` (the overwhelming default) means
/// the [`crate::sky`] gradient dome is the backdrop. Resolved from [`CameraInteriorClaim`]: the same
/// down-ray seed that already names the camera's room for the MFOG fog resolve.
#[derive(Resource, Default, PartialEq, Eq)]
pub struct CameraSkybox(pub Option<String>);

/// The resolved skybox's slot WEIGHT this frame — the second half of the resolve, and the number
/// that makes the sky *crossfade* instead of pop (the report this closes). Per the byte law
/// (module header):
///
/// - the **WMO slot's** weight is `[0xce9bdc]`, the camera-in-WMO interior crossfade — THE same
///   number the MFOG fog lerp rides ([`crate::lighting::WmoCrossfade`], ±0.25/s, a 4-second fade
///   in and out). Engagement is the fog's own (claim + MFOG count > 1 + a true-interior
///   containing group), so at Stratholme's gate the painted sky fades in over the celestial pass
///   exactly as the storm veil fades out of the streets;
/// - the **DBC/ghost slot's** weight is hardcoded `1.0` whenever it is filled (`0x6d2260`) — the
///   ghost sky pops, faithfully;
/// - `0.0` when no skybox resolves — including a skybox *named* by the flood while the crossfade
///   sits at 0 (standing outside the city gate: the flood reaches flagged groups through the
///   doorway and publishes the name, but the reference draws a slot only at weight > 0, so no
///   painted sky shows until you are actually in).
///
/// Consumers: [`apply_skybox_visibility`] (draw gate `weight > 0` = `0x6d4afe`, and the per-batch
/// alpha), and the celestial stand-down gates in [`crate::sky`], [`crate::sun`] and
/// [`crate::clouds`] via [`Self::replaces_celestial`].
#[derive(Resource, Default, PartialEq)]
pub struct SkyboxWeight(pub f32);

impl SkyboxWeight {
    /// Does the skybox replace the whole celestial pass this frame? `weight > 0.99`
    /// (`[0x808aac]`, tested at `0x6d49e8`/`0x6d4a1f`): below it the six celestial elements
    /// still draw and the skybox alpha-blends OVER them — that compositing is the visible half
    /// of the 4-second crossfade.
    pub fn replaces_celestial(&self) -> bool {
        self.0 > 0.99
    }
}

/// Marks one batch of a built skybox model, tagged with the model path it belongs to (a session can
/// walk through more than one skybox building, and the built entities are cached, not rebuilt).
#[derive(Component)]
struct SkyboxPart {
    /// The skybox model path this batch belongs to.
    path: String,
    /// The batch's authored-blend material — what it draws with at weight 1.0.
    steady: Handle<WowModelMaterial>,
    /// The blend-promotion twin for `0 < weight < 1` (`0x811fe0`; equal to
    /// `steady` for a batch whose authored mode already blends).
    fade_blend: Handle<WowModelMaterial>,
}

/// This batch **spins**: every one of its vertices is wholly weighted to one parentless
/// rotation-only bone, so the authored motion is a rigid transform about that bone's pivot and needs
/// no skinning palette at all ([`benilla_formats::BoneSpin`]).
///
/// That is the whole reason the belts can turn here rather than on the `M2Model` asset lane, which
/// is what decision 1264 assumed this would cost. A rig would want joint entities under the anchor,
/// and the anchor is written *post-propagation* ([`follow_camera`], decision 0504's same-frame
/// camera pose) — so its children's globals would be a frame stale, and fixing that means either
/// giving up the same-frame pose or hand-writing joint globals. A rigid batch sidesteps the whole
/// question: there are no children.
///
/// **Population, measured** (`benilla-extract m2batch`/`m2bones`): `CavernsOfTimeSky.m2`'s four
/// asteroid-belt batches, `[1@1.00]×38`, `[2@1.00]×38`, `[3@1.00]×38` and `[3@1.00]×28`, on three
/// parentless bones keying rotation alone over one 66.667 s loop. The other 17 batches ride bone 0,
/// which has no track, and `StratholmeSkybox.m2` has no animation at all — so this is the entire
/// animated content of every skybox the chain ships.
#[derive(Component)]
struct SkyboxSpin {
    /// The bone's pivot, in the same (Bevy) space the batch's vertices were baked into.
    pivot: Vec3,
    spin: benilla_formats::BoneSpin,
}

/// Which skybox paths have been built already — a build is a chain read + BLP decodes, so it happens
/// once per path per session and the entities are then just shown/hidden. A path that FAILED to load
/// is recorded here too: the retry would fail identically every frame, and the gradient dome is the
/// correct thing to fall back to.
#[derive(Resource, Default)]
struct BuiltSkyboxes(HashSet<String>);

/// Ordering handle: [`CameraSkybox`] is settled for the frame after this set. `crate::sky`'s dome
/// gate hangs off it, because the two backdrops must agree *within* a frame — one reading a stale
/// resource is a frame with both drawn or neither.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SkyboxResolve;

/// The WMO-skybox subsystem: resolve which model the camera's room wants, build it on first need,
/// then show exactly that one and pin it to the camera.
pub(crate) struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        // No `MaterialPlugin` of its own any more: a skybox batch is a `WowModelMaterial` like every
        // other model batch, and that plugin is already loaded.
        app.init_resource::<CameraSkybox>()
            .init_resource::<SkyboxWeight>()
            .init_resource::<BuiltSkyboxes>()
            .add_systems(
                Update,
                (resolve_camera_skybox, build_skybox, apply_skybox_visibility)
                    .chain()
                    // The claim we read is written by the PVS pass in this same schedule. Unordered,
                    // this reads whichever side of it the executor happened to pick — so a camera
                    // move that changes rooms lands a frame late, or not at all on the frame it
                    // matters, and the backdrop flickers between the painted sky and the gradient.
                    .after(crate::wmo_portal::WmoPvsSet)
                    // …and the slot weight is this frame's crossfade `t`, advanced by the lighting
                    // resolve — a stale read here is a frame where the sky's alpha and the interior
                    // fog disagree about how far into the building the camera is.
                    .after(crate::lighting::LightingResolveSet)
                    .in_set(SkyboxResolve),
            )
            // Camera-anchored placement runs post-propagation off the SAME-frame camera pose — the
            // slot decision 0504 moved every camera-anchored shell into.
            .add_systems(
                PostUpdate,
                follow_camera.in_set(crate::billboard::BillboardPlace),
            );
    }
}

/// Which skybox does this frame ask for? **The flag is tested on the groups the portal flood
/// REACHES, never on the group the camera stands in** (`0x6b42e0`, inside the flood `0x6b41c0`, with
/// `ebx` = the group being visited). So the predicate is: *does any group in this placement's PVS
/// carry `SHOW_SKYBOX`, and does its root name a MOSB?*
///
/// That distinction is the whole bug this replaced. Reading the **containing** group instead is
/// wrong at Stratholme's King's Square: the BSP containment there is group 39, the root's only
/// EXTERIOR (`0x8`) group, which does not set `SHOW_SKYBOX` — yet the reference draws the painted
/// sky, because 61 of the 83 groups the flood reaches from group 39 do carry the bit. (Verified on
/// the asset: BFS over `Stratholme_B`'s MOPR reaches 82 of 83 groups from group 39, 61 of them
/// flagged.) The camera's down-ray CLAIM is a different resolver and is *not* cleared there — at
/// the reported spot it seeds the street group g08 (`0x42805`, a true interior; pin-probed
/// 2026-09-01, decision 1827) — which is what lets the crossfade weight engage in the square while
/// the skybox *name* comes from the flood predicate above.
fn resolve_camera_skybox(
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    sampler: Option<Res<crate::lighting::LightSampler>>,
    viewer: Res<crate::view::Viewer>,
    current_map: Option<Res<crate::world_map::CurrentMap>>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    crossfade: Res<crate::lighting::WmoCrossfade>,
    mut want: ResMut<CameraSkybox>,
    mut weight: ResMut<SkyboxWeight>,
) {
    // **The ghost sky first — it wins outright.** Slot B is filled at weight 1.0 and that makes the
    // reference skip loop A entirely (module header), so a dead player under
    // Stratholme's painted sky sees DeathClouds, not the building's.
    //
    // Resolved from the SAME (map, camera position) seed the atmosphere uses, so the sky and the
    // fog it hangs over come from one zone row. The ghost bit is `PLAYER_FLAGS` bit 0x10 — the same
    // one `lighting::resolve` reads to select param slot 4 — so the two switch on the same frame.
    if viewer.ghost {
        let ghost_sky = sampler.as_ref().and_then(|s| {
            let pos = bevy_to_wow(cam.single().ok()?.translation());
            let map = current_map.as_ref().map_or(0, |m| m.0);
            s.0.ghost_skybox(map, pos)
        });
        if let Some(sky) = ghost_sky {
            if want.0.as_deref() != Some(sky) {
                want.0 = Some(sky.to_owned());
            }
            // Slot B's weight is hardcoded 1.0 whenever it is filled (`0x6d26cb`/`0x6d26d0`):
            // the ghost sky pops in whole, and — being > 0.99 — replaces the celestial pass on
            // the same frame the death atmosphere lands.
            weight.set_if_neq(SkyboxWeight(1.0));
            return;
        }
    }
    // `min()` rather than "first match": `Query` iteration order is not stable across frames, and a
    // tie would otherwise alternate two backdrops frame to frame. Five roots qualify — Stratholme_B
    // and the four Caverns of Time shells — and the tie-break is live code, not a formality: the
    // note that used to sit here read "unreleased" as "unreachable" and concluded this can never
    // pick twice. `CavernsofTime.wmo` is placed in the live world (decision 1264, and the module
    // header above); two of its shells overlapping the camera is exactly the case `min()` settles.
    let resolved = instances
        .iter()
        .filter_map(|inst| {
            let model = wmos.get(&inst.handle)?;
            // The MOSB test comes first and exits ~every instance in one deref: 810 of the chain's
            // 815 roots name no skybox, so the group scan below is effectively never reached.
            let sky = model.skybox.as_deref()?;
            model
                .group_nav
                .iter()
                .enumerate()
                .any(|(i, nav)| {
                    // NOT the fail-open `unwrap_or(true)` the rest of the cull takes
                    // ([`WmoGroupVis::drawn_by`]): there a lookup miss must never blank a building,
                    // whereas here it would paint a full-screen sky over the entire world.
                    nav.flags & SHOW_SKYBOX != 0 && inst.visible.get(i).copied().unwrap_or(false)
                })
                .then(|| sky.to_owned())
        })
        .min();
    if want.0 != resolved {
        want.0 = resolved;
    }
    // The WMO slot draws at the interior crossfade's weight — `[0xce9bdc]`, the SAME number the
    // MFOG fog lerp rides — and at 0.0 when nothing resolved. A resolved name at weight 0 is a
    // real state (the flood publishes through a doorway the camera hasn't walked into), and it is
    // exactly the reference's: the slot is filled, and `0x6d4afe` declines to draw it.
    weight.set_if_neq(SkyboxWeight(match want.0 {
        Some(_) => crossfade.t(),
        None => 0.0,
    }));
}

/// Build the wanted skybox's entities the first time it is asked for. The models are small (3 batches
/// for Stratholme, 21 for Caverns of Time) and there are five in the whole game, so a built one is
/// kept for the session rather than torn down on leaving the room — walking in and out of
/// Stratholme's gate must not re-decode art.
fn build_skybox(
    mut commands: Commands,
    want: Res<CameraSkybox>,
    mut built: ResMut<BuiltSkyboxes>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: M2BatchMaterials,
) {
    let Some(path) = want.0.as_deref() else {
        return;
    };
    if built.0.contains(path) {
        return;
    }
    // The shared light buffer is created in the render world's shadow at startup; a build that ran
    // before it would bake materials against nothing. Retry — and do NOT latch `built` here, or the
    // first frame in the room would permanently give up on the sky.
    if !mats.ready() {
        return;
    }
    let Some(mut world_assets) = world_assets else {
        return; // assetless dev run — the gradient dome stays the backdrop
    };
    // The model's rigid spins, keyed by bone — empty for `StratholmeSkybox` and for a capture
    // (`deterministic_run`), which keeps every animated lane on its bind pose so world baselines stay
    // comparable across runs and branches, exactly as the doodad host's own arm does.
    let spins = if crate::dev_state::deterministic_run() {
        Default::default()
    } else {
        benilla_formats::load_m2_bone_spins(&mut world_assets.chain.lock_recover(), path)
            .unwrap_or_default()
    };
    let subs = benilla_formats::load_m2_mesh(&mut world_assets.chain.lock_recover(), path);
    let subs = match subs {
        Ok(subs) if !subs.is_empty() => subs,
        Ok(_) => {
            warn!("skybox '{path}' has no render batches — keeping the gradient dome");
            built.0.insert(path.to_string());
            return;
        }
        Err(e) => {
            warn!("skybox '{path}' failed to load, keeping the gradient dome: {e:#}");
            built.0.insert(path.to_string());
            return;
        }
    };
    for (i, sub) in subs.iter().enumerate() {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        let positions: Vec<[f32; 3]> = sub
            .positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        // The model lane's vertex stage declares `world_normal` unconditionally and fills it under
        // `VERTEX_NORMALS`; the sky is unlit and never reads it, but the attribute has to be there
        // for the layout the shared shader was compiled against.
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            sub.normals
                .iter()
                .map(|n| wow_to_bevy(*n).to_array())
                .collect::<Vec<_>>(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
        mesh.insert_indices(Indices::U32(sub.indices.clone()));
        // The batch's own authored address mode (decision 0763) — a skybox's UVs sit inside 0..1 for
        // the cube faces, but Caverns of Time's asteroid belts tile theirs over ±21 wraps.
        let texture = sub
            .texture
            .as_deref()
            .and_then(|t| world_assets.texture(t, (sub.wrap_x, sub.wrap_y), &mut images));
        // `i + 1` is the authored-batch-order convention (`0` = unordered): the transparent half of
        // a skybox is camera-anchored, so every one of its batches shares a sort distance and the
        // order is the only thing keeping the layers from re-flipping every frame.
        let Some(pair) = mats.skybox(sub, texture, u16::try_from(i + 1).unwrap_or(0)) else {
            return; // light buffer vanished mid-build; `built` is unlatched, so we retry
        };
        let part = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(pair.steady.clone()),
                Transform::default(),
                Visibility::Hidden, // `apply_skybox_visibility` turns on exactly the wanted one
                // The per-instance alpha the crossfade writes (bits 0..=5, the one field this
                // population uses — no rig, no shade/probe, no flags; `apply_skybox_visibility`
                // is its sole writer).
                MeshTag(crate::mesh_tag::spawn_tag(0, 1.0)),
                SkyboxPart {
                    path: path.to_string(),
                    steady: pair.steady,
                    fade_blend: pair.fade_blend,
                },
            ))
            .id();
        if let Some(spin) = sole_bone(sub).and_then(|b| spins.get(&b)) {
            commands.entity(part).insert(SkyboxSpin {
                pivot: wow_to_bevy(spin.pivot),
                spin: spin.clone(),
            });
        }
    }
    built.0.insert(path.to_string());
}

/// The bone this batch is **wholly** weighted to, if any — the caller's half of
/// [`benilla_formats::BoneSpin`]'s rigid-body condition, and the one half the bone table cannot
/// answer: a vertex split between this bone and another is half a rigid body's worth of motion,
/// which no single transform can produce.
///
/// `None` for a batch with no skin binding at all (every WMO batch, and any loader that didn't fill
/// it) — the conservative answer, since an unweighted batch has nothing to spin about.
fn sole_bone(sub: &benilla_formats::RenderSubmesh) -> Option<u16> {
    let bone = sub.joints.first()?[0];
    (sub.weights.len() == sub.joints.len()
        && sub
            .joints
            .iter()
            .zip(&sub.weights)
            // `> 0.999` rather than `== 1.0`: the weights are the file's 0..255 bytes normalised by
            // their sum, so a wholly-bound vertex is exactly 1.0 today — but an equality test on a
            // divided float is the kind of thing that silently stops matching.
            .all(|(j, w)| j[0] == bone && w[0] > 0.999))
    .then_some(bone)
}

/// Show the wanted skybox's batches at this frame's slot weight and hide every other built one —
/// the draw half of the 4-second crossfade. This is the sole `Visibility`, material and `MeshTag`
/// writer for these entities (the gradient dome's own gate lives in [`crate::sky`], which reads
/// [`SkyboxWeight`] — one authority per entity class, decision 0025).
///
/// Per slot the reference draws only at weight > 0 (`0x6d4afe fcomp 0.0`), with the weight
/// multiplied into every batch's combined alpha (`0x710cb0` → `[CM2Model+0x180]`)
/// — and a batch at `0 < A < 1` is PROMOTED to
/// SRC_ALPHA/INV_SRC_ALPHA blending whatever its authored mode (`0x811fe0`),
/// which is what lets an opaque painted cube fade. Here that is: weight ≤ 0 ⇒ hidden; weight < 1
/// ⇒ the blend-promotion twin + the weight in the `MeshTag` alpha field (the same rails every
/// entity feather rides); weight ≥ 1 ⇒ the steady authored material, alpha field full.
fn apply_skybox_visibility(
    want: Res<CameraSkybox>,
    weight: Res<SkyboxWeight>,
    mut parts: Query<(
        &SkyboxPart,
        &mut Visibility,
        &mut MeshMaterial3d<WowModelMaterial>,
        &mut MeshTag,
    )>,
) {
    for (part, mut vis, mut mat, mut tag) in &mut parts {
        let show = weight.0 > 0.0 && want.0.as_deref() == Some(part.path.as_str());
        let target = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *vis != target {
            *vis = target;
        }
        if !show {
            continue;
        }
        let handle = if weight.0 < 1.0 {
            &part.fade_blend
        } else {
            &part.steady
        };
        if mat.0 != *handle {
            mat.0 = handle.clone();
        }
        let bits = crate::mesh_tag::with_alpha(tag.0, weight.0);
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
}

/// Pin the box to the camera, world-aligned (identity rotation) so the painted sky stays fixed to the
/// world horizon however the camera turns — the same treatment [`crate::sky::follow_camera`] gives the
/// gradient dome, minus its far-plane scaling.
///
/// The art is drawn at **authored scale** and that is deliberate. Every other shell here scales to a
/// fraction of the far plane, but those radii were only ever standing in for occlusion, and the forced
/// far depth ([`crate::sky_order`], "The depth law") retired that job. Scale is genuinely free here:
/// the eye's offset inside the box scales with the box, so the *angles* the faces subtend — which is
/// all a camera-anchored backdrop can show — are scale-invariant.
///
/// **The ANCHOR POINT is not free — and it is the model's ORIGIN, VERIFIED.** `CM2Model+0xbc` stays
/// identity and `0x707680` is called with a **zeroed** recentre vector (`0x6d4b3a`–`0x6d4b48`) where
/// the world M2 scene gets the live camera position — so the model's local origin sits exactly at the
/// eye, with camera rotation and zero translation. (The capture agrees: the skybox's matrix is
/// orthonormal with translation `(0,0,0)` while the very next model's carries a real offset.)
///
/// It matters because `StratholmeSkybox` is *not* centred on its origin: its 52.87 yd cube sits at
/// z ∈ [−14.67, +38.20], leaving the eye 11.76 yd BELOW the box's centre. That asymmetry is authored,
/// not incidental — the near-black ±Z pair covers only a **34.7°** cone about the zenith rather than
/// a symmetric 45°, and the side pairs' painted horizon lands three-quarters down their gradient
/// (v ≈ 0.72). Re-centring the box on the eye would be a visible change and a wrong one.
/// A spinning batch ([`SkyboxSpin`]) composes its bone's rotation about the bone's own pivot INSIDE
/// the anchor: `T(eye) · T(pivot) · R(t) · T(−pivot)`, which closes to the rotation itself plus the
/// translation below. The pivot conjugation is not decoration — Caverns of Time's belt bones pivot
/// ~3 yd off the model origin, and the eye sits AT that origin ~20 yd from the belt, so dropping it
/// swings the whole ring through several degrees instead of turning it in place.
///
/// The clock is the scene's own elapsed time, wrapped by the sequence. **The phase origin is not
/// byte-pinned** — the reference arms the model at load and samples a shared clock, so its phase is
/// whatever the room's load moment was — and for a 66.7 s loop of an asteroid ring with no start
/// event, phase is unobservable. Captures never reach here at all (the spin component is not
/// attached under a deterministic run), so no golden frame depends on it.
#[allow(clippy::type_complexity)]
fn follow_camera(
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    mut parts: Query<
        (&mut Transform, &mut GlobalTransform, Option<&SkyboxSpin>),
        (With<SkyboxPart>, Without<WorldCamera>),
    >,
) {
    let Some(cam_gt) = cam.iter().next() else {
        return;
    };
    let now = time.elapsed_secs();
    for (mut tf, mut gt, spin) in &mut parts {
        let (rot, pivot) = match spin {
            Some(s) => (
                benilla_assets::coords::wow_rotation_to_bevy(s.spin.sample(now)),
                s.pivot,
            ),
            None => (Quat::IDENTITY, Vec3::ZERO),
        };
        tf.translation = cam_gt.translation() + pivot - rot * pivot;
        tf.rotation = rot;
        tf.scale = Vec3::ONE;
        // Propagation already ran this frame — the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer's half of the rigid-spin predicate, on the real art: exactly the four
    /// asteroid-belt batches resolve to a bone the collector spins, and the other seventeen resolve
    /// to bone 0 — which is *weighted* wholly, and simply has no track.
    ///
    /// The distinction matters and is why this asserts both columns. `sole_bone` returning `Some(0)`
    /// for the painted cube is correct and harmless; it is the SPIN lookup that must come back empty
    /// for it. A test that only checked "four parts spin" would pass just as well if the predicate
    /// were selecting the wrong four.
    #[test]
    fn only_the_belt_batches_of_the_caverns_sky_resolve_to_a_spinning_bone() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::Chain::open(&data).expect("open vanilla patch chain");
        const SKY: &str = "Environments\\Stars\\CavernsOfTimeSky.m2";
        let subs = benilla_formats::load_m2_mesh(&mut chain, SKY).expect("load the sky");
        let spins = benilla_formats::load_m2_bone_spins(&mut chain, SKY).expect("its spins");

        let bones: Vec<Option<u16>> = subs.iter().map(sole_bone).collect();
        assert_eq!(bones.len(), 21, "21 authored batches");
        // Batches 5..=8 are the belts (`m2batch`: `[1@1.00]×38`, `[2@1.00]×38`, `[3@1.00]×38`,
        // `[3@1.00]×28`); every other batch rides bone 0.
        assert_eq!(
            &bones[5..=8],
            &[Some(1), Some(2), Some(3), Some(3)],
            "the belt batches and the bones they ride"
        );
        assert!(
            bones
                .iter()
                .enumerate()
                .filter(|(i, _)| !(5..=8).contains(i))
                .all(|(_, b)| *b == Some(0)),
            "every non-belt batch is wholly on bone 0: {bones:?}"
        );

        let spinning = bones
            .iter()
            .filter(|b| b.and_then(|b| spins.get(&b)).is_some())
            .count();
        assert_eq!(spinning, 4, "exactly the four belt batches turn");
    }
}
