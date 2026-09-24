//! M2 → model asset loader.
//!
//! Decodes a WoW M2 (`*.m2`) into an [`M2Model`]: one [`ModelSubmesh`] per render batch, each
//! carrying the batch's **decoded geometry** plus its texture as a `Handle<Image>` **dependency**
//! (through the `mpq://` source). **The loader ships no meshes** (decision 0834, the model-lane
//! twin of 0832's terrain rule): a labeled mesh sub-asset lands the whole model's render form in
//! ONE frame — the city first-contact spike — so the app builds each batch's `Mesh` paced at the
//! spawn side (`benilla`'s `model_forms`, via [`submesh_to_static_mesh`](crate::submesh_to_static_mesh)
//! / [`submesh_to_skinned_mesh`](crate::submesh_to_skinned_mesh)).
//!
//! Deliberately app-independent: ships **geometry + texture handles + metadata only**, not materials
//! (the WoW lighting material is an app concern built at spawn) — so this asset is shared verbatim by
//! doodads, creatures, GameObjects, and the star model. Creature **skin variations** (`Monster1/2/3`)
//! are blank here (no `skins` passed) and resolved at the spawn site from `CreatureDisplayInfo`; this
//! loads the model's own embedded (hardcoded) textures.
//!
//! References use `.mdx`/`.mdl`, but the physical archive file is `.m2`; callers map the extension when
//! forming the `mpq://…m2` load path (the loader registers for `m2`).

use std::sync::Arc;

use benilla_formats::{
    hand_grip_finger_poses, parse_m2_animations, parse_m2_attachments, parse_m2_bounds,
    parse_m2_collision_hull, parse_m2_global_sequence_bones, parse_m2_lights,
    parse_m2_particle_emitters, parse_m2_playable_animation_lookup, parse_m2_portrait_camera,
    parse_m2_render_submeshes, parse_m2_skeleton, CollisionMesh, M2Bounds, M2Light,
    ParticleEmitterDef,
};
use bevy::animation::graph::AnimationGraph;
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::reflect::TypePath;

use crate::bone_target_id;
use crate::coords::wow_to_bevy;
use crate::model::{
    arm_subtree_roots, billboard_info, build_animation_clip, build_attachments, build_global_bones,
    build_grip_clip, build_skeleton, finger_subtree_roots, in_subtree, skeleton_pivots,
    upper_subtree_root, AnimClip, ModelAnimations, ModelAttachment, ModelSkeleton, ModelSubmesh,
};

/// A loaded M2 model: its render batches + authored bounds (the distance-fade size source) + the
/// coarse collision hull (the app bakes a collider from it per placement) + its particle emitters.
#[derive(Asset, TypePath, Clone)]
pub struct M2Model {
    pub submeshes: Vec<ModelSubmesh>,
    /// Authored bounding sphere/box (model-local yards) — the reference's distance-fade size. `None`
    /// if the header bounds couldn't be read.
    pub bounds: Option<M2Bounds>,
    /// The doodad's collision hull in raw WoW model space (trunk/solid; ≪ render mesh). `None` when the
    /// model carries no hull (`nBoundingTriangles == 0` — many small props are collide-iff-hull).
    pub collision: Option<CollisionMesh>,
    /// Particle emitters (flames, glows, smoke). Each carries its parsed def (positions in raw WoW
    /// model space) + its resolved texture handle. Empty for most models; one or two for campfires.
    pub emitters: Vec<ModelEmitter>,
    /// Ribbon emitters (weapon trails, wisp streamers, missile trails — the reference's
    /// `CRibbonEmitter`, simulated per frame at `0x7b7e60`). Same shape as the particle emitters:
    /// parsed def + resolved trail texture + the host bone's pivot for joint riding. Empty for
    /// nearly everything (176 models corpus-wide author one).
    pub ribbons: Vec<ModelRibbon>,
    /// M2 light blocks (raw WoW model space + the host bone's rest pivot). The spawn site turns each
    /// casting `type==1` (point) light into a Bevy `PointLight` that lays the faithful dynamic
    /// hot-spot on every lit surface (decisions 0016/0273). Empty for nearly all models; one on fire
    /// props (campfire/firepit/cooker/forge) — and one on the held torch every torch-bearing NPC
    /// carries.
    pub lights: Vec<ModelLight>,
    /// The model's rest skeleton in Bevy space (decision 0019) — the joint tree the skinned creature
    /// path spawns one entity per bone from. Empty for a boneless model.
    pub skeleton: ModelSkeleton,
    /// The matching inverse bind poses (`translate(−pivot)` per bone), shared across every instance of
    /// this model; each skinned submesh references it from its `SkinnedMesh`. A labeled sub-asset.
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// The model's animations, ready to play (decision 0019) — one shared `AnimationGraph` with every
    /// sequence's clip. `None` for a model with no animated sequences (static props, boneless models).
    /// Built once here; each creature instance drives it with its own `AnimationPlayer`.
    pub animations: Option<ModelAnimations>,
    /// The **file-order-first sequence's authored duration** (seconds, one full pass) — carried even
    /// when no clip builds ([`ModelAnimations`] drops sequences that move no bone, and a model whose
    /// sequences all sit still has no `animations` at all). The real client's effect-completion
    /// clock runs one pass of the armed sequence regardless of bone keys or the LOOP flag (the fire
    /// is the CM2 advance `0x7194f8`, a pure end-boundary compare; loop is tested only *after* it)
    /// — the spell-fx self-termination reads this. Named approximation: the client arms
    /// `animationLookup[0]`'s sequence, not strictly the file-order-first one; they coincide on
    /// every effect model probed (single-sequence models). `None` for a model with no sequences at
    /// all.
    pub first_seq_span: Option<f32>,
    /// **Every sequence the file owns, in file order** — id, slot, length, loop — whatever
    /// [`Self::animations`] kept a clip for. The UI model pane's clock needs exactly this
    /// (decision 2008): which `AnimationData` ids the file answers to, how long each runs and
    /// whether it clamps — for sequences that key no bone too, which `ModelAnimations` drops
    /// (the cooldown indicator's sweep keys a constant on its one bone; its texture transforms
    /// and colour tracks are what move). Empty for a file with no sequences.
    pub sequences: Vec<M2SequenceInfo>,
    /// The model's attachment points (decision 0072 — held items): weapon/shield hand slots, sheath
    /// points, etc. Each carries the bone it rides + its bind-pose-relative offset (see
    /// [`ModelAttachment`]). Empty for a model with no attachment table (most doodads/WMO props).
    pub attachments: Vec<ModelAttachment>,
    /// The animation-event table's positional markers (`$CSL`/`$CSR`/`$CST` casting hands, `$BWR`
    /// ranged release, …), baked like [`Self::attachments`] and queried by 4CC first-match — the
    /// client's `0x7130e0`/`0x7131b0` surface. Empty for a model with no events.
    pub markers: Vec<crate::ModelMarker>,
    /// The model's authored **portrait camera** (`benilla_formats::M2PortraitCamera` baked to Bevy
    /// space) — the exact rig the real client renders the unit-frame portrait through
    /// (`cameraLookup[0]` via `0x713540`, `lookAt` + authored perspective at `0x7ac640`, no
    /// engine-side framing on top). `None` for a model with no camera table (props, a few
    /// creatures).
    pub portrait_camera: Option<PortraitCamera>,
    /// The camera **table's first record** — the glue screens' `Model:SetCamera(0)` rig (the
    /// create/select background scenes; their camera *lookup* is the 0xffff none sentinel, so
    /// `portrait_camera` sees nothing there). Same Bevy-space conversion as `portrait_camera`.
    pub camera0: Option<PortraitCamera>,
    /// The camera table's **second** record — what a 1.12 `<PlayerModel>` UI widget (the paper doll,
    /// the inspect pane, the pet page) renders through (`0x505b30` → the chooser `0x505890` selects
    /// **raw index 1**, the `type == 1` "characterinfo" camera, and freezes it at `0x7acf10`).
    ///
    /// **Raw, not through `cameraLookup`** — that array is the portrait bake's path and is not
    /// consulted here; the index is a literal 1 whatever the record's `type` says. `None` for a
    /// model with fewer than two cameras, where the client synthesizes a *fixed* camera instead
    /// (transcribed at the framing site). Same Bevy-space conversion as `portrait_camera`.
    pub pane_camera: Option<PortraitCamera>,
    /// The **whole camera table**, in file order — the raw index space a `<Model>` widget's
    /// `SetCamera(n)` walks (decision 2027). `camera0` and `pane_camera` above are the two fixed
    /// indices two other paths take; this is the general one, and it is the only one that can
    /// answer "how many cameras does this file have", which is the question that decides whether
    /// a pane renders through the perspective leg at all (an index past the count installs a NULL
    /// camera and the pane falls back to the orthographic leg).
    pub cameras: Vec<PaneCamera>,
    /// A bow's `$WTT`/`$WTB` bowstring anchors (the endpoints of the string drawer `0x611ff0`),
    /// baked to Bevy space: `[top, bottom]` as `(bone index, model-local position)` — the two
    /// limb-tip points the engine-drawn string spans. `None` for every non-bow model.
    pub string_anchors: Option<[(u16, Vec3); 2]>,
    /// The fishing pole's `$CCH` line anchor (read by the line draw `0x61f780`), baked to Bevy
    /// space in the mesh frame — the rod-tip point the engine-drawn fishing line starts from.
    /// `None` for every model that doesn't author it (exactly one weapon model in the chain does).
    pub cch_marker: Option<Vec3>,
    /// MD20 header `GlobalModelFlags` (`+0x10`). Bits `&3` are the **terrain-conform gate**
    /// (read by the root-matrix builder `0x7106c0`): `1` = pitch to the ground slope (every
    /// mount + most quadrupeds), `3` = pitch **and** roll (kodo/crab/spider), `0`/`2` = level.
    /// The entity layer reads it to conform a standing model to the terrain under it.
    pub global_flags: u32,
}

/// One sequence of an M2, as the file table has it — see [`M2Model::sequences`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct M2SequenceInfo {
    /// The `AnimationData.dbc` id (`M2Sequence+0x00`).
    pub anim_id: u16,
    /// The sequence's file slot — the axis the per-sequence material bakes are keyed on
    /// ([`benilla_formats::ModelAnimation::seq_index`]).
    pub seq_index: usize,
    /// `end − start` on the file's timeline, ms.
    pub duration_ms: u32,
    /// The band's **start** on the file's global timeline, ms (`M2Sequence+0x04`) — what a
    /// per-sequence cursor is added to before an absolute-timeline track is sampled. The camera
    /// tracks are the one consumer: their keys are absolute like every other M2 track's, while a
    /// pane's play head is a cursor inside the armed band.
    pub start_ms: u32,
    /// Loops (flag bit 0 clear); a clamped sequence holds its last frame.
    pub looping: bool,
}

/// An M2's authored portrait-camera rig in Bevy space (see [`M2Model::portrait_camera`]): eye/target
/// in model-local yards at scale 1 (the client bakes portraits with root scale reset to 1), the
/// projection scalars straight from the record.
#[derive(Clone, Copy)]
pub struct PortraitCamera {
    pub eye: Vec3,
    pub target: Vec3,
    /// Roll about the eye→target axis (radians) — rotates the up vector; `0.0` on every portrait
    /// camera audited (kept for fidelity, not observed nonzero).
    pub roll: f32,
    /// The record's FOV (radians), verbatim — a **diagonal** angle in the client's convention;
    /// the consumer builds the client's diagonal-FOV projection from it at whichever aspect its
    /// path uses (`benilla`'s `WowPortraitProjection`) — `1.0` for the round portrait bake, so
    /// `fovy = fov/√2` (decision 1543), the pane's own rect for a `<PlayerModel>` (1089).
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// One record of the file's camera table, by **raw index** — see [`M2Model::cameras`].
///
/// The rest rig ([`Self::still`]) is Bevy space like every other camera on this asset; the
/// authored tracks stay raw WoW space behind [`Self::at`], which does the conversion, so there is
/// no mixed-convention field to misread.
#[derive(Clone)]
pub struct PaneCamera {
    /// The record's `type` word — `0` portrait, `1` "characterinfo", `-1` on the fly-bys and the
    /// six glue scenes. Carried, never selected on: the widget's index is raw.
    pub camera_type: i32,
    /// The rig at rest (bases + each track's first key), Bevy space. The whole answer for every
    /// model a `<Model>` pane can name — a census over the composite's 9691 `.m2` found every one
    /// of those cameras keying a single `(0,0,0)` on all three tracks.
    pub still: PortraitCamera,
    /// The authored tracks, raw WoW space, kept only when one of them actually moves — the
    /// `Cameras\*.m2` fly-bys. Read through [`Self::at`].
    tracks: Option<Arc<benilla_formats::M2CameraTracks>>,
}

impl PaneCamera {
    /// The rig at absolute file-timeline `ms`, Bevy space — [`Self::still`] for a static camera,
    /// the cubically sampled tracks for a moving one (`benilla_m2::M2Track::sample_ms`, the
    /// reference's own four-way `interp` dispatch).
    ///
    /// `wow_to_bevy` is linear, so converting the summed `base + track(t)` is the same as
    /// converting each — which is why the tracks can stay raw behind this one door.
    pub fn at(&self, ms: u32) -> PortraitCamera {
        let Some(t) = self.tracks.as_deref() else {
            return self.still;
        };
        let add = |b: [f32; 3], v: Option<[f32; 3]>| {
            let v = v.unwrap_or([0.0; 3]);
            wow_to_bevy([b[0] + v[0], b[1] + v[1], b[2] + v[2]])
        };
        PortraitCamera {
            eye: add(t.position_base, t.positions.sample_ms(ms)),
            target: add(t.target_base, t.target.sample_ms(ms)),
            roll: t.roll.sample_ms(ms).unwrap_or(self.still.roll),
            ..self.still
        }
    }
}

/// The **billboard frame** an emitter's bone chain reaches — the host bone's arm, its pivot in raw
/// WoW model space, and **which bone it is**. See [`ModelEmitter::billboard`] for when it is `Some`
/// and why the emitter's live origin is then `pivot + camBasis·(def.position − pivot)`.
///
/// The bone index is what lets a **rigged** consumer be exactly right rather than approximately:
/// seating the replacement frame on that bone's own joint puts it at the composed pivot with the
/// chain above it already folded in, where placing it at `pivot` under the model root silently
/// assumes every ancestor is at rest.
#[derive(Clone, Copy, Debug)]
pub struct EmitterBillboard {
    pub kind: benilla_formats::BillboardKind,
    /// The host bone's pivot, **raw WoW model space**.
    pub pivot: [f32; 3],
    /// The host bone's index — its seat in the skeleton, for a consumer that has a rig.
    pub bone: u16,
}

/// One particle emitter of an [`M2Model`]: the parsed [`ParticleEmitterDef`] plus its resolved
/// particle texture as a `Handle<Image>` dependency (loaded through the `mpq://` source, like a
/// submesh albedo). The app's particle system simulates + renders it at the spawn site.
#[derive(Clone)]
pub struct ModelEmitter {
    pub def: ParticleEmitterDef,
    pub texture: Option<Handle<Image>>,
    /// The host bone's (`def.bone`) pivot, **raw WoW model space** (0130 phase 4). `def.position` is
    /// model-space; `position − bone_pivot` is the same point in the bone's own frame — the offset
    /// an emitter riding a live joint composes through the joint's transform. Rest pose is identity
    /// about the pivot, so the static path (`placement · position`) is the exact special case.
    /// `[0; 3]` when the bone index is out of range (or the model is boneless).
    pub bone_pivot: [f32; 3],
    /// The **billboard frame** this emitter rides, when its bone chain reaches a billboard bone
    /// (`benilla_formats::Skeleton::billboard_host`): that bone's arm and its pivot, **raw WoW
    /// model space**. The emitter's live origin is then camera-dependent —
    /// `pivot + camBasis·(def.position − pivot)` — because the reference folds the record position
    /// through the *replaced* palette matrix (`0x718960` at `0x7190a9`–`0x71910c`; the billboard
    /// tail `0x715868` writes that matrix).
    ///
    /// A rigged host in the WORLD needs nothing from this: its joint palette already carries the
    /// replacement (`billboard::billboard_joint_palette`) and the emitter rides the joint. Two
    /// lanes still do: the spawn paths with **no rig** — an equipped item's model
    /// (helm/shoulders/held), where the whole affected population's chains are otherwise rest-pose
    /// (corpus-verified: 95 item models / 197 emitters ride a billboard chain, **zero** with an
    /// animating one — `benilla-extract bbscan`) — and every **booth** bake, rigged or not, whose
    /// pose deliberately drops the camera arms (`RigPose::without_camera_billboards`: the world
    /// pass would face them at the *world* camera, which is not the one drawing a booth). `None`
    /// for the ordinary case.
    pub billboard: Option<EmitterBillboard>,
    /// The **recursion model** (`def.recursion_model`) as a loaded dependency: its own emitters
    /// become this emitter's CHILD emitters (the reference's load-completion wiring `0x7b5dd0`),
    /// wired by the app's particle system once the asset resolves. `None` when unauthored.
    pub recursion: Option<Handle<M2Model>>,
    /// The **geometry model** (`def.geometry_model`) as a loaded dependency: this emitter's
    /// particles render as 3-D instances of it instead of billboard quads (the per-particle
    /// instance draw `0x7b4840`). `None` when unauthored.
    pub geometry: Option<Handle<M2Model>>,
    /// How far the OWNER model's own transparent-pass batches sort from its origin, model-local
    /// yards — the bound the draw-order rung is sized from (`particles::owner_last_bias`,
    /// decisions 0719/0721). Computed by [`benilla_formats::m2_owner_reach`], which is where the
    /// reasoning lives.
    pub owner_reach: f32,
    /// The OWNER model's authored bound sphere — the water-plane classification input
    /// (`0x7083df`–`0x7084dd`): the reference dots `world_matrix × (bbox_min+bbox_max)/2` against
    /// the plane once per MODEL and every emitter reads that one verdict, with the slack
    /// `above ⇔ d ≥ −r`, `r = |matrix row 0| × sphere radius`. Centre in **Bevy model space** (the
    /// same frame the instance transform maps), radius model-local yards. `(ZERO, 0)` when the
    /// header carries no bounds — the sign test at the anchor.
    pub water_bound: (Vec3, f32),
    /// The OWNER model's **loader-idle file sequence slot**
    /// ([`crate::ModelAnimations::idle_seq`]) — the sequence every M2 instance is playing when no
    /// rig has armed anything, and therefore the slot this emitter's per-sequence rate/gate/param
    /// bakes sample against by default (decision 0936). Baked onto the emitter, not asked of the
    /// spawn site, because the lanes that need it most are exactly the ones with no rig to ask:
    /// a content-gated GameObject (`spawn_emitter`'s `EmitClock::Host` whose player armed nothing)
    /// and an unrigged placed doodad (`EmitClock::Pinned`). `0` for a model with no sequences —
    /// the same degrade as before, now stated once instead of falling out of an `unwrap_or`.
    pub idle_seq: usize,
}

/// Whether looping `anim` would look like anything other than the **static mesh** — the doodad
/// content gate (decision 0130, premise corrected by 0637).
///
/// The original gate asked "does some track have >1 key", reasoning that a constant pose renders
/// identically to the un-rigged mesh. That premise is false: the static mesh is the **bind** pose,
/// and a constant clip only equals it when the constant *is* rest. `DuelingFlag.m2` is the
/// counter-example that cost us the bug — its Stand band brackets to a constant bone-0 translation
/// of `−9.124` (the flag planted in the ground) while its bind pose is 9 yards up in the air, so a
/// "constant ⇒ skip the rig" gate renders it floating.
///
/// So: a track that MOVES qualifies, and so does a constant sitting away from rest — translation
/// ≠ 0 (the M2 translation track is a delta on the pivot offset), rotation ≠ identity, scale ≠ 1.
/// Genuinely rest-posed idles — the ~90% of placed doodads decision 0130 measured — still take the
/// static path, so that optimization survives intact.
///
/// The predicate itself is `ModelAnimation::is_rest_pose`, beside the parse, so
/// `benilla-extract idleslotscan`'s corpus census of this gate's reach can't drift from the gate
/// (decision 0936). What it decides is **whether to build a rig**, and nothing else — the sequence
/// the instance is playing is [`ModelAnimations::idle_seq`], which has an answer either way.
fn idle_pose_differs(anim: &benilla_formats::ModelAnimation) -> bool {
    !anim.is_rest_pose()
}

/// A model reference inside an M2 record (`.mdx`/`.mdl`, mixed case, backslashes) → its
/// `mpq://…m2` load URL — the same extension remap the client's loader does (the physical
/// archive file is `.m2`).
fn m2_dep_url(raw: &str) -> String {
    let p = raw.to_ascii_lowercase().replace('\\', "/");
    let stem = p
        .strip_suffix(".mdx")
        .or_else(|| p.strip_suffix(".mdl"))
        .or_else(|| p.strip_suffix(".m2"))
        .unwrap_or(&p);
    format!("mpq://{stem}.m2")
}

/// One ribbon emitter of an [`M2Model`]: the parsed [`benilla_formats::RibbonEmitterDef`] plus its
/// resolved trail texture and — like [`ModelEmitter::bone_pivot`] — the host bone's rest pivot, so
/// the spawn site can ride the bone's joint with `position − pivot` in the joint frame (the
/// reference transforms the bone-local position by the live bone matrix each frame, `0x718960`).
#[derive(Clone)]
pub struct ModelRibbon {
    pub def: benilla_formats::RibbonEmitterDef,
    pub texture: Option<Handle<Image>>,
    pub bone_pivot: [f32; 3],
    /// The owner model's reach — same field, same bound, same reason as
    /// [`ModelEmitter::owner_reach`]. A trail is one of its model's emitters and takes the same
    /// owner-last draw-order rung: measured on the wisp (14 blend batches, 3 streamers), its
    /// trails otherwise interleave with its own body, 6 batches deep, and which batches are over
    /// which streamer *changes frame to frame* as they whip (decision 0721).
    pub owner_reach: f32,
    /// The owner model's bound sphere for the water-plane side — same field, same law as
    /// [`ModelEmitter::water_bound`]: the ribbon leg reads the MODEL's side-A boolean verbatim
    /// (`cmp [ebp-0x24],0` at `0x7081f1`), slack included.
    pub water_bound: (Vec3, f32),
}

/// One M2 light block of an [`M2Model`]: the raw [`M2Light`] record plus — like
/// [`ModelEmitter::bone_pivot`] — its host bone's rest pivot, so a light on an ANIMATING model can
/// ride that bone's joint with `position − pivot` in the joint frame. The reference does exactly
/// this every frame (`0x718960` transforms the def position by the live bone matrix before
/// registering the light), which is how a torch's glow tracks the hand that swings it.
#[derive(Clone, Copy)]
pub struct ModelLight {
    pub def: M2Light,
    pub bone_pivot: [f32; 3],
}

/// Bevy [`AssetLoader`] decoding `*.m2` → [`M2Model`].
#[derive(Default, TypePath)]
pub struct M2ModelLoader;

impl AssetLoader for M2ModelLoader {
    type Asset = M2Model;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<M2Model, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let to_io = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));

        // `dir` only feeds creature skin-variation resolution, which is inert here (we pass no skins —
        // skins are applied at the spawn site from CreatureDisplayInfo), so an empty dir is correct.
        let subs = parse_m2_render_submeshes(&bytes, "", &[]).map_err(to_io)?;
        let bounds = parse_m2_bounds(&bytes).ok();
        // The owner-last draw-order bound, once for the whole model: every effect this model
        // authors (quad emitters, ribbon trails, model-particle instances) takes the rung it sizes,
        // because the reference draws them all in the one post-batch bracket.
        let owner_reach = benilla_formats::m2_owner_reach(&subs);
        // The water-plane classification sphere, once for the whole model — the header AABB's
        // midpoint (Bevy model frame) + the header sphere radius. See [`ModelEmitter::water_bound`].
        let water_bound = bounds.as_ref().map_or((Vec3::ZERO, 0.0), |b| {
            (
                wow_to_bevy([
                    (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                    (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                    (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
                ]),
                b.sphere_radius,
            )
        });
        // The collision hull (collide-iff-hull): keep only a non-empty mesh so the spawn site can skip
        // hull-less props cheaply.
        let collision = parse_m2_collision_hull(&bytes)
            .ok()
            .filter(|c| !c.is_empty());

        let mut submeshes = Vec::with_capacity(subs.len());
        for sub in subs {
            // No meshes built here (decision 0834): the geometry ships on the submesh and the app
            // builds the render form paced — the static mesh on demand, the skinned twin only for
            // the lanes that rig (which also retires the old always-built, mostly-unused twin).
            //
            // Embedded texture → an mpq:// dependency (WorldArt, the loader default). Lowercase the
            // path: Bevy's loader lookup is case-sensitive, and these tables carry uppercase `.BLP`
            // (the `BlpImageLoader` registers `blp`), so an uppercase extension silently falls back to
            // type-based resolution — which is ambiguous with Bevy's built-in image loader and spams a
            // "Multiple AssetLoaders found" warning per texture. Lowercasing also dedupes case variants.
            // The URL carries the batch's authored sampler address mode, because two modes of one
            // `.blp` are two different `Image` uploads (decision 0763 — `crate::texture_url`).
            let texture = sub
                .texture
                .as_deref()
                .map(|t| ctx.load::<Image>(crate::texture_url(t, (sub.wrap_x, sub.wrap_y))));
            submeshes.push(ModelSubmesh {
                texture,
                skin_slot: sub.skin_slot,
                geoset_id: sub.geoset_id,
                char_slot: sub.char_slot,
                icon_slot: sub.icon_slot,
                blend: sub.blend,
                two_sided: sub.two_sided,
                interior: false,        // M2 has no interior/exterior group concept
                emissive: sub.emissive, // M2 UNLIT (0x01) glass/glow → fullbright
                sidn: None,             // WMO-only (MOMT SIDN night glow)
                window: false,          // WMO-only (MOMT WINDOW interior light)
                additive: sub.additive, // M2 blend 3/4 → additive (glow cards)
                env_map: sub.env_map, // texture_unit_lookup > 2 → the runtime generates this stage's UVs
                no_depth_write: sub.no_depth_write, // M2 render flag 0x10
                no_depth_test: sub.no_depth_test, // M2 render flag 0x08
                fog_policy: sub.fog_policy, // M2 render flag 0x02 / the per-blend fog table
                billboard: billboard_info(&sub), // glow cards / chains face the camera
                alpha_anim: sub.alpha_anim.clone().map(std::sync::Arc::new),
                uv_anim: sub.uv_anim.clone().map(std::sync::Arc::new),
                uv_seq: sub.uv_seq.clone().map(std::sync::Arc::new),
                uv_rot_seq: sub.uv_rot_seq.clone().map(std::sync::Arc::new),
                uv_scale_seq: sub.uv_scale_seq.clone().map(std::sync::Arc::new),
                rgb_anim: sub.rgb_anim.clone().map(std::sync::Arc::new),
                rgb_seq: sub.rgb_seq.clone().map(std::sync::Arc::new),
                wmo_batch: None,                // M2 batches have no MOBA section
                ground_quad: sub.ground_quad(), // flat ground-ring quads → the fx decal lane
                geometry: std::sync::Arc::new(sub),
            });
        }
        // M2 light blocks (raw — wow-m2's light parse reads the wrong vanilla record shape). The spawn
        // site filters to casting point lights ([`M2Light::casts`]). Empty for nearly all models.
        // The bone-pivot bake is below, once the skeleton is parsed.
        let light_defs = parse_m2_lights(&bytes);

        // The rest skeleton (decision 0019): bake the raw bone tree to Bevy-space joints + the matching
        // inverse bind poses. A boneless model yields an empty skeleton + an empty bindpose set (the
        // skinned mesh then carries no joint attributes, so it never takes the SKINNED path).
        let skeleton_raw = parse_m2_skeleton(&bytes).unwrap_or_default();

        // Particle emitters: resolve each emitter's texture to an `mpq://` image dependency (same
        // lowercase normalisation as submesh albedos), and bake its host bone's pivot so the spawn
        // site can ride a live joint (see [`ModelEmitter::bone_pivot`]). Absent on most models.
        let mut emitters: Vec<ModelEmitter> = parse_m2_particle_emitters(&bytes)
            .unwrap_or_default()
            .into_iter()
            .map(|def| {
                let texture = def.texture.as_deref().map(|t| {
                    ctx.load::<Image>(format!(
                        "mpq://{}",
                        t.replace('\\', "/").to_ascii_lowercase()
                    ))
                });
                let bone_pivot = skeleton_raw
                    .bones
                    .get(def.bone as usize)
                    .map_or([0.0; 3], |b| b.pivot);
                // The recursion/geometry models load as ordinary mpq:// M2 dependencies —
                // children wiring and model-particle instancing happen app-side on resolve.
                let recursion = def
                    .recursion_model
                    .as_deref()
                    .map(|p| ctx.load::<M2Model>(m2_dep_url(p)));
                let geometry = def
                    .geometry_model
                    .as_deref()
                    .map(|p| ctx.load::<M2Model>(m2_dep_url(p)));
                // The billboard frame the emitter's bone chain inherits, if any (see the field).
                let billboard = skeleton_raw.billboard_host(def.bone).and_then(|h| {
                    let b = skeleton_raw.bones.get(h)?;
                    Some(EmitterBillboard {
                        kind: b.billboard?,
                        pivot: b.pivot,
                        bone: u16::try_from(h).ok()?,
                    })
                });
                ModelEmitter {
                    def,
                    texture,
                    bone_pivot,
                    billboard,
                    recursion,
                    geometry,
                    owner_reach,
                    water_bound,
                    idle_seq: 0, // stamped below, once the sequences are built
                }
            })
            .collect();

        // Ribbon emitters (trails): same texture resolution, bone-pivot bake and owner-reach bake
        // as the particles — a trail is one of the model's emitters and draws under the same law.
        let ribbons: Vec<ModelRibbon> = benilla_formats::parse_m2_ribbon_emitters(&bytes)
            .unwrap_or_default()
            .into_iter()
            .map(|def| {
                let texture = def.texture.as_deref().map(|t| {
                    ctx.load::<Image>(format!(
                        "mpq://{}",
                        t.replace('\\', "/").to_ascii_lowercase()
                    ))
                });
                let bone_pivot = skeleton_raw
                    .bones
                    .get(def.bone as usize)
                    .map_or([0.0; 3], |b| b.pivot);
                ModelRibbon {
                    def,
                    texture,
                    bone_pivot,
                    owner_reach,
                    water_bound,
                }
            })
            .collect();

        // M2 lights: the same host-bone pivot bake, so a light on an animating model (the torch in
        // an NPC's hand) can ride its bone's joint instead of freezing at the rest-pose spot.
        let lights = light_defs
            .into_iter()
            .map(|def| ModelLight {
                bone_pivot: skeleton_raw
                    .bones
                    .get(def.bone as usize)
                    .map_or([0.0; 3], |b| b.pivot),
                def,
            })
            .collect();
        let (skeleton, inverse_bindposes) = build_skeleton(&skeleton_raw);
        let inverse_bindposes = ctx.add_labeled_asset(
            "inverse_bindposes".to_string(),
            SkinnedMeshInverseBindposes::from(inverse_bindposes),
        );

        // The attachment-point table (decision 0072 — held items): the same bind-pose pivots
        // `build_skeleton` used above, so a held item's offset is exactly consistent with the joint
        // it's spawned under. Empty for a boneless/attachment-less model.
        let attachments = build_attachments(
            &parse_m2_attachments(&bytes).unwrap_or_default(),
            &skeleton_pivots(&skeleton_raw),
        );
        // The event-marker table, same bake (the missile-launch $CSL/$CSR/$CST/$BWR points).
        let markers = crate::model::build_markers(
            &benilla_formats::parse_m2_event_markers(&bytes).unwrap_or_default(),
            &skeleton_pivots(&skeleton_raw),
        );
        // …and the same pivots again for the per-clip event keys below ([`crate::ClipEvent`]),
        // so a fired key's point is bake-consistent with the joint it composes through.
        let pivots = skeleton_pivots(&skeleton_raw);

        // The animations (decision 0019): build each sequence's clip into one shared `AnimationGraph`
        // (the bench scrubs them; gameplay plays Stand). All as labeled sub-assets. `None` unless at
        // least one sequence produced an animated clip.
        let sequences = parse_m2_animations(&bytes);
        let first_seq_span = sequences.first().map(|a| a.duration).filter(|d| *d > 0.0);
        let sequence_infos: Vec<M2SequenceInfo> = sequences
            .iter()
            .map(|a| M2SequenceInfo {
                anim_id: a.anim_id,
                seq_index: a.seq_index,
                duration_ms: a.end_ms.saturating_sub(a.start_ms),
                start_ms: a.start_ms,
                looping: a.looping,
            })
            .collect();
        let animations = {
            let mut graph = AnimationGraph::new();
            let root = graph.root;
            // The direct pose evaluator's bake (decision 0712) — filled beside every graph
            // mutation below so the table mirrors the graph by construction: `bone_masks` beside
            // each `add_target_to_mask_group`, `set_node` beside each `add_clip*`.
            let mut pose = crate::model::PoseSource {
                bone_masks: vec![0u64; skeleton_raw.bones.len()],
                ..Default::default()
            };
            // Per-arm mask groups for the client's per-slot one-shots (the draw/stow plays each
            // hand's ceremony on its own arm subtree, over the gait). Each arm's subtree root is the
            // **shoulder-keybone ancestor of the hand attachment's bone** (attachment id 1 = right
            // hand, 2 = left; keybones 2/3 = the shoulders — data-driven, no left/right guessing).
            // Every joint *outside* an arm joins that arm's mask group, so a node masked with group
            // 0 (or 1) animates only the right (or left) arm.
            let arm_roots = arm_subtree_roots(&skeleton_raw, &attachments);
            if let Some((right_root, left_root)) = arm_roots {
                for i in 0..skeleton_raw.bones.len() {
                    let target = bone_target_id(i as u16);
                    if !in_subtree(&skeleton_raw, i, right_root) {
                        graph.add_target_to_mask_group(target, 0);
                        pose.bone_masks[i] |= 1 << 0;
                    }
                    if !in_subtree(&skeleton_raw, i, left_root) {
                        graph.add_target_to_mask_group(target, 1);
                        pose.bone_masks[i] |= 1 << 1;
                    }
                }
            }
            // The upper-body one-shot mask group (decision 0087): group 2 collects every joint
            // *outside* the SpineLow subtree (the legs + pelvis), so a clip masked with it animates
            // only the upper body — the general form of the per-arm ceremony above, one subtree wider.
            let upper_root = upper_subtree_root(&skeleton_raw);
            if let Some(upper_root) = upper_root {
                for i in 0..skeleton_raw.bones.len() {
                    if !in_subtree(&skeleton_raw, i, upper_root) {
                        graph.add_target_to_mask_group(bone_target_id(i as u16), 2);
                        pose.bone_masks[i] |= 1 << 2;
                    }
                }
            }
            // Per-hand **finger** mask groups (3 = right hand, 4 = left) for the weapon grip: a clip
            // masked with group 3 (or 4) animates only that hand's finger key-bone subtrees, so the
            // `HandsClosed` pose curls the fingers while the arm keeps its gait (`CloseHand`
            // `0x479660`). Empty per hand for a model with no finger key-bones (beasts).
            let finger_roots = finger_subtree_roots(&skeleton_raw);
            for i in 0..skeleton_raw.bones.len() {
                let target = bone_target_id(i as u16);
                if !finger_roots[0].is_empty()
                    && !finger_roots[0]
                        .iter()
                        .any(|&r| in_subtree(&skeleton_raw, i, r))
                {
                    graph.add_target_to_mask_group(target, 3);
                    pose.bone_masks[i] |= 1 << 3;
                }
                if !finger_roots[1].is_empty()
                    && !finger_roots[1]
                        .iter()
                        .any(|&r| in_subtree(&skeleton_raw, i, r))
                {
                    graph.add_target_to_mask_group(target, 4);
                    pose.bone_masks[i] |= 1 << 4;
                }
            }
            // The model's baked PlayableAnimationLookup (decision 0082): parsed straight off the M2
            // header alongside the sequences above, so a model lacking a requested clip can resolve
            // to its own baked substitute at play time (`ModelAnimations::resolve`) instead of the
            // creature driver silently doing nothing. Read BEFORE the clip walk because the
            // loader-idle seed below resolves through it.
            let playable_animation_lookup =
                parse_m2_playable_animation_lookup(&bytes).unwrap_or_default();
            // The model's own "do I author id X" table (the reference's `0x711960`) — the
            // GameObject arm's missing-sequence remap branches on it (`ModelAnimations::owns`).
            let animation_lookup =
                benilla_formats::parse_m2_animation_lookup(&bytes).unwrap_or_default();
            let mut clips = Vec::new();
            // The loader-idle seed (decision 0637, `0x71019b`): the loader arms **animation id 0
            // ("Stand")** resolved through the model's own `playableAnimationLookup`, NOT the
            // file-order-first sequence. For the overwhelming majority of models those coincide,
            // which is why the old file-order read survived; they diverge exactly on models whose
            // first sequence is a Spawn — `DuelingFlag.m2` is Spawn(145)/Stand(0)/Despawn(157), and
            // looping its Spawn band left the flag hanging 9 yards in the air on a 3.3 s cycle.
            let idle_id = playable_animation_lookup
                .first()
                .map_or(0, |p: &benilla_formats::PlayableAnim| p.resolved_id);
            let mut first_seq = None;
            for (i, anim) in sequences.iter().enumerate() {
                // EVERY sequence becomes a clip — the clip carries the instance's sequence CLOCK,
                // not just a bone pose (decision 0941, [`build_animation_clip`]). `poses_bones`
                // separates the two meanings: the clock is free, the rig is not.
                {
                    let (clip, pose_clip, poses_bones) = build_animation_clip(anim, &skeleton);
                    // `first_seq` stays the RENDERING question — "is this seed worth a skin +
                    // player" (0130's content gate, and decision 0936's split). The identity
                    // question, "which sequence does the loader arm", is
                    // [`ModelAnimations::idle_seq`]/`idle_clip`, which asks the same selection
                    // without the gate.
                    if first_seq.is_none() && anim.anim_id == idle_id && idle_pose_differs(anim) {
                        first_seq = Some(clips.len());
                    }
                    let pose_idx = pose.clips.len() as u32;
                    pose.clips.push(pose_clip);
                    let clip = ctx.add_labeled_asset(format!("clip{i}"), clip);
                    let node = graph.add_clip(clip.clone(), 1.0, root);
                    pose.set_node(node, pose_idx, 0);
                    // The sheath family gets per-arm masked variants ([`AnimClip::arm_nodes`]).
                    let arm_nodes =
                        (matches!(anim.anim_id, 89 | 90) && arm_roots.is_some()).then(|| {
                            let nodes = (
                                graph.add_clip_with_mask(clip.clone(), 1 << 0, 1.0, root),
                                graph.add_clip_with_mask(clip.clone(), 1 << 1, 1.0, root),
                            );
                            pose.set_node(nodes.0, pose_idx, 1 << 0);
                            pose.set_node(nodes.1, pose_idx, 1 << 1);
                            nodes
                        });
                    // The upper-body masked variant ([`AnimClip::upper_node`]): the masked destination
                    // for a one-shot routed over a live base (swing/emote — the route is chosen per play
                    // in `creature_anim`). Built for every clip when the model has a split key-bone, so
                    // any one-shot id can take the masked route; `None` on the −1-sentinel models.
                    let upper_node = upper_root.map(|_| {
                        let n = graph.add_clip_with_mask(clip.clone(), 1 << 2, 1.0, root);
                        pose.set_node(n, pose_idx, 1 << 2);
                        n
                    });
                    clips.push(AnimClip {
                        anim_id: anim.anim_id,
                        seq_index: anim.seq_index,
                        node,
                        looping: anim.looping,
                        duration: anim.duration,
                        move_speed: anim.move_speed,
                        blend_time: anim.blend_time,
                        // Into the mesh vertices' frame, so the pick's world math is one transform.
                        bounds_center: wow_to_bevy(anim.bounds_center),
                        bounds_radius: anim.bounds_radius,
                        // The axis mapping flips signs, so min/max are re-derived componentwise.
                        bounds_min: wow_to_bevy(anim.bounds_min).min(wow_to_bevy(anim.bounds_max)),
                        bounds_max: wow_to_bevy(anim.bounds_min).max(wow_to_bevy(anim.bounds_max)),
                        // The kernel's own quantity, baked (see [`ClipEvent`]): the bone-local
                        // offset for a rigged model and the model-space point for one whose bone
                        // chain never moves — both from the record's OWN bone, never a by-4CC
                        // re-find, because a model may author the tag more than once.
                        events: anim
                            .events
                            .iter()
                            .map(|e| crate::ClipEvent {
                                time: e.time,
                                ident: e.ident,
                                data: e.data,
                                bone: e.bone,
                                offset: wow_to_bevy(e.position)
                                    - pivots.get(e.bone as usize).copied().unwrap_or(Vec3::ZERO),
                                point: wow_to_bevy(e.position),
                            })
                            .collect::<Vec<_>>()
                            .into(),
                        arm_nodes,
                        upper_node,
                        frequency: anim.frequency,
                        replay: (anim.min_replay, anim.max_replay),
                        poses_bones,
                    });
                }
            }
            // The per-hand weapon-grip nodes ([`ModelAnimations::hand_close`]): a purpose-built grip clip
            // — each finger key-bone posed at its **clamped** `HandsClosed` value — masked to one hand's
            // finger subtrees (group 3 = right, 4 = left). Built separately from the sequence clips above
            // because the general in-band read drops the weapon-hand fingers (keyed off the HandsClosed
            // frame); [`hand_grip_finger_poses`] clamps them, scoped to the grip (`CloseHand`
            // `0x479660`). `None` per hand for a model with no finger key-bones / no grip pose.
            let mut hand_close: [Option<bevy::animation::graph::AnimationNodeIndex>; 2] =
                [None, None];
            // Every bone in a finger subtree — the key-bone AND its child segments (the fingertip joints),
            // so the whole finger curls, not just its base (HandsClosed keys the tips too, ~51° vs ~30°).
            let finger_bones: Vec<u16> = (0..skeleton_raw.bones.len())
                .filter(|&i| {
                    finger_roots
                        .iter()
                        .flatten()
                        .any(|&r| in_subtree(&skeleton_raw, i, r))
                })
                .map(|i| i as u16)
                .collect();
            let grip_poses = hand_grip_finger_poses(&bytes, &finger_bones);
            if !grip_poses.is_empty() {
                let (grip_clip, grip_pose) = build_grip_clip(&grip_poses);
                let grip_idx = pose.clips.len() as u32;
                pose.clips.push(grip_pose);
                let grip = ctx.add_labeled_asset("grip_clip".to_string(), grip_clip);
                if !finger_roots[0].is_empty() {
                    let n = graph.add_clip_with_mask(grip.clone(), 1 << 3, 1.0, root);
                    pose.set_node(n, grip_idx, 1 << 3);
                    hand_close[0] = Some(n);
                }
                if !finger_roots[1].is_empty() {
                    let n = graph.add_clip_with_mask(grip.clone(), 1 << 4, 1.0, root);
                    pose.set_node(n, grip_idx, 1 << 4);
                    hand_close[1] = Some(n);
                }
            }
            // Global-sequence channels alone are enough to carry `ModelAnimations`: a model whose
            // sequences produced no clips can still pulse/flicker on its free-running loops (the
            // doodad `GlobalSeqOnly` tier, decision 0130) — dropping them because `clips` came up
            // empty would silently freeze it.
            let global_bones =
                build_global_bones(&parse_m2_global_sequence_bones(&bytes), &skeleton);
            // The gate is "does ANYTHING in this model vary with the sequence clock" — and its
            // consumer list was incomplete (decision 0941). A bone-posing clip and a global
            // sequence were counted; the **per-sequence samplers** were not: the emitters'
            // rate/enable/params tracks, a ribbon's, and the material alpha/colour/UV loops all
            // resolve through the instance's playing sequence. 807 corpus models animate ONLY
            // through those, and denying them `ModelAnimations` denied them the clock itself —
            // no player, no slot, no time — so every one of those tracks read file slot 0 at
            // t=0 for ever. Models with none of the three keep the static path: the optimization
            // decision 0130 measured (~90% of placed doodads) is untouched, and 6342 boneless
            // models with nothing to sample stay out of the animation lane entirely.
            let samples_sequence = !emitters.is_empty()
                || !ribbons.is_empty()
                || submeshes
                    .iter()
                    .any(|s| s.alpha_anim.is_some() || s.uv_anim.is_some() || s.rgb_anim.is_some());
            (clips.iter().any(|c| c.poses_bones)
                || !global_bones.is_empty()
                || (samples_sequence && !clips.is_empty()))
            .then(|| {
                let graph = ctx.add_labeled_asset("anim_graph".to_string(), graph);
                ModelAnimations {
                    graph,
                    clips,
                    playable_animation_lookup,
                    animation_lookup,
                    hand_close,
                    global_bones,
                    first_seq,
                    pose: std::sync::Arc::new(pose),
                }
            })
        };

        // Stamp every emitter with the model's loader-idle slot (decision 0936) — the sequence a
        // rig-less instance is playing, and so the slot its rate/gate/param bakes sample by
        // default. Done here rather than in the map above because the selection needs the built
        // clips (their `seq_index` + the `playableAnimationLookup` resolve), and only `None` when
        // the model builds no clip at all — nothing to name, so the historical slot 0 stands.
        let idle_seq = animations.as_ref().and_then(ModelAnimations::idle_seq);
        for e in &mut emitters {
            e.idle_seq = idle_seq.unwrap_or(0);
        }

        // The authored portrait camera (raw WoW model space → Bevy, same pure rotation the meshes
        // bake through, so eye/target land in exactly the submeshes' frame) — and the camera
        // table's first record, the glue screens' `SetCamera(0)` rig.
        let to_bevy = |c: benilla_formats::M2PortraitCamera| PortraitCamera {
            eye: wow_to_bevy(c.position),
            target: wow_to_bevy(c.target),
            roll: c.roll,
            fov: c.fov,
            near: c.near_clip,
            far: c.far_clip,
        };
        let portrait_camera = parse_m2_portrait_camera(&bytes).map(to_bevy);
        let camera0 = benilla_formats::parse_m2_camera(&bytes, 0).map(to_bevy);
        let pane_camera = benilla_formats::parse_m2_camera(&bytes, 1).map(to_bevy);
        // The whole table, raw index — the `<Model>` widget's `SetCamera(n)` space (2027).
        let cameras: Vec<PaneCamera> = benilla_formats::parse_m2_pane_cameras(&bytes)
            .into_iter()
            .map(|c| PaneCamera {
                camera_type: c.camera_type,
                still: to_bevy(c.still),
                tracks: c.tracks.map(|t| Arc::new(*t)),
            })
            .collect();

        // The bowstring anchors (bows only): raw WoW positions → Bevy, the meshes' frame.
        let string_anchors = benilla_formats::parse_m2_string_anchors(&bytes).map(|a| {
            [
                (a.top.0, wow_to_bevy(a.top.1)),
                (a.bottom.0, wow_to_bevy(a.bottom.1)),
            ]
        });

        // The fishing line's near anchor (mesh frame, like the bowstring anchors).
        let cch_marker = benilla_formats::parse_m2_cch_marker(&bytes).map(|(_, p)| wow_to_bevy(p));

        // The header GlobalModelFlags word sits at a fixed offset (MD20 magic @0, +0x10) — a
        // bare scalar the sectioned parsers never touch.
        let global_flags = bytes
            .get(0x10..0x14)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));

        Ok(M2Model {
            submeshes,
            bounds,
            collision,
            emitters,
            ribbons,
            lights,
            skeleton,
            inverse_bindposes,
            animations,
            first_seq_span,
            sequences: sequence_infos,
            attachments,
            markers,
            portrait_camera,
            camera0,
            pane_camera,
            cameras,
            string_anchors,
            cch_marker,
            global_flags,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["m2"]
    }
}
