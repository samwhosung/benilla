//! The plain M2 data types: the parsed record shapes (vertices, textures, materials, bones,
//! attachments, the authored bounds) and the top-level [`M2Model`]/[`M2Format`] the parser
//! (`crate::parse_m2`, in `lib.rs`) builds.

use std::ffi::CString;

use crate::track::{
    M2QuatTrack, M2ScalarSplineTrack, M2ScalarTrack, M2Vec3SplineTrack, M2Vec3Track,
};

/// A 3-float vector (position / normal / pivot).
#[derive(Clone, Copy)]
pub struct C3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
/// A 2-float vector (texture coordinate).
#[derive(Clone, Copy)]
pub struct C2 {
    pub x: f32,
    pub y: f32,
}

/// M2 texture purpose. Vanilla uses `Hardcoded` (the `filename` is the real path) or the `Monster*`
/// skin slots (filename usually empty; the slot is filled from the creature's display variation).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum M2TextureType {
    Hardcoded,
    Monster1,
    Monster2,
    Monster3,
    Other(u32),
}

impl M2TextureType {
    pub(crate) fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Hardcoded,
            11 => Self::Monster1,
            12 => Self::Monster2,
            13 => Self::Monster3,
            other => Self::Other(other),
        }
    }
}

/// The texture's filename, kept as a `CString` so callers can `.string.to_string_lossy()` (matching
/// the reference's shape).
pub struct M2ArrayString {
    pub string: CString,
}

/// One M2 texture definition.
pub struct M2Texture {
    pub texture_type: M2TextureType,
    /// Address mode on U (`flags & 0x1`): set = the sampler REPEATS, clear = it CLAMPS to edge.
    pub wrap_x: bool,
    /// Address mode on V (`flags & 0x2`).
    pub wrap_y: bool,
    pub filename: M2ArrayString,
}

/// M2 material render flags (`& 0x01` = unlit/emissive, `& 0x04` = two-sided).
#[derive(Clone, Copy)]
pub struct M2RenderFlags(pub(crate) u16);
impl M2RenderFlags {
    pub fn bits(&self) -> u16 {
        self.0
    }
}
/// M2 material blend mode (0 opaque, 1 alpha-key, 2 alpha, 3/4 additive).
#[derive(Clone, Copy)]
pub struct M2BlendMode(pub(crate) u16);
impl M2BlendMode {
    pub fn bits(&self) -> u16 {
        self.0
    }
}

/// One M2 material (MD20 "render flags" entry).
pub struct M2Material {
    pub flags: M2RenderFlags,
    pub blend_mode: M2BlendMode,
}

/// M2 bone flags (`& 0x08` spherical billboard, `& 0x10/0x20/0x40` cylindrical billboard).
#[derive(Clone, Copy)]
pub struct M2BoneFlags(pub(crate) u32);
impl M2BoneFlags {
    pub fn bits(&self) -> u32 {
        self.0
    }
}

/// One M2 bone. The render path reads billboard flags + pivot; the skeletal-animation path also needs
/// the **parent index** (`-1` = root) to compose bone matrices up the tree and to build the joint
/// hierarchy + inverse bind poses (decision 0019). Vanilla bone record is stride 108: keyBoneId i32
/// @+0x00, flags @+0x04, parent i16 @+0x08, pivot C3 @+0x60 (no boneNameCRC, unlike TBC+).
pub struct M2Bone {
    /// The bone's `KeyBoneID` (`-1` = none): the semantic anchor table — 0/1 arms L/R, 2/3
    /// shoulders L/R, 4 spine-low, 5 waist, 6 head… The per-arm animation masks resolve their
    /// subtree roots through it.
    pub key_bone: i16,
    pub flags: M2BoneFlags,
    /// Parent bone index, or `-1` for a root bone (vanilla bone record `+0x08`).
    pub parent: i16,
    pub pivot: C3,
}
impl M2Bone {
    /// Whether this bone billboards (any of the spherical/cylindrical bits).
    pub fn is_billboard(&self) -> bool {
        self.flags.0 & (0x08 | 0x10 | 0x20 | 0x40) != 0
    }
}

/// One M2 vertex (48 bytes on disk). Layout: position C3 @0, bone_weights [u8;4] @0x0c,
/// bone_indices [u8;4] @0x10, normal C3 @0x14, tex_coords C2 @0x20 (a second UV @0x28 we skip). The
/// weights + indices are the 4-bone skin binding the skeletal-animation path uploads (decision 0019);
/// the static render path uses only `bone_indices[0]` (billboard glow-card pivot).
pub struct M2Vertex {
    pub position: C3,
    /// Per-vertex bone influence weights (`/255`), paired with [`Self::bone_indices`].
    pub bone_weights: [u8; 4],
    pub bone_indices: [u8; 4],
    pub normal: C3,
    pub tex_coords: C2,
}

/// Lookup tables + collision geometry read from the header. The renderer uses the texture-combo →
/// texture mapping; benilla-formats reads the collision hull from the two `bounding_*` byte blocks
/// (the MD20 `boundingVertices` are `C3Vector`s = 12 bytes; `boundingTriangles` are `u16` = 2 bytes —
/// kept as raw bytes so callers slice them as they need).
pub struct M2RawData {
    pub texture_lookup_table: Vec<u16>,
    /// Collision-hull triangle indices, raw bytes (`count × 2`, `u16` each).
    pub bounding_triangles: Vec<u8>,
    /// Collision-hull vertices, raw bytes (`count × 12`, `C3Vector` each).
    pub bounding_vertices: Vec<u8>,
}

/// The authored bounds from the MD20 header. Two boxes: the **bounding** box (+ sphere radius) is the
/// render/cull volume (the all-animation vertex extent — large); the **collision** box (+ sphere radius)
/// is the tight collision hull (~the body). The **render sphere** sizes the *unit* selection ring
/// (`0.5 × radius × scale`, the client's cached `[unit+0x2b0]`) and drives render culling; the
/// **collision box** is the *GameObject* ring's source (`0x711a20`/`0x713700`) — parsed here for
/// that path.
pub struct M2Header {
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub collision_box_min: [f32; 3],
    pub collision_box_max: [f32; 3],
    pub collision_sphere_radius: f32,
}

/// One row of the M2's baked **PlayableAnimationLookup** table (decision 0082, `0x711bf0`): the
/// model's own precomputed answer to "if the game requests `AnimationData.dbc` id X, which id do I
/// actually play, and in which direction/variant". Header `count@+0x2c`/`offset@+0x30` (pre-Wrath
/// M2 only — the array is dropped past version 263), stride 4 (one dword per row): low16 = the
/// resolved id, high16 = a direction/variant playback code. `nPlayableAnimationLookup` is a fixed
/// **203** across the entire retail 1.12.1 corpus (measured) — the per-model table is a baked cache
/// of the `AnimationData.dbc` Fallback-column walk (PATH 2, below), frozen against that model's
/// actual sequence set at export (a chicken lacking Attack2H bakes `[18] = [17] = 16`).
#[derive(Clone, Copy)]
pub struct M2PlayableAnim {
    /// The `AnimationData.dbc` id this model actually plays for the row's requested id.
    pub resolved_id: u16,
    /// Direction/variant playback code — **not yet consumed** by benilla (decision 0082 flags its
    /// exact playback semantics, `~0x7126d2`, as untraced; carried here for a future slice).
    pub dir_flags: u16,
}

/// One M2 attachment-point record (vanilla stride 48: `id u32 @+0`, `bone u32 @+4`, `position C3 @+8`,
/// then a 28-byte visibility `M2Track` we skip). `id`/`bone` are narrowed to `u16` — safe on real data
/// (attachment ids run ≤ 36; `bone` is checked against the bone array during parse) — and any record
/// that doesn't fit, or whose bone is out of the model's bone-array range, is dropped rather than kept
/// malformed.
///
/// Empirical ground truth (direct dump of `HumanMale.m2`, `decisions/0072`): on character models the
/// attachment `position` equals its bone's pivot — the attach bones are leaves sitting exactly at the
/// attach point, so `position` and the bone pivot agree. Known ids: `0` shield (left forearm), `1`
/// right hand, `2` left hand, `26`/`27` right/left back sheath, `28` shield back, `30`/`31` lower-back
/// pair, `32`/`33` hip pair.
#[derive(Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    /// Model-space position, **raw WoW space** — matches how [`M2Bone::pivot`] is stored; the WoW→Bevy
    /// bake happens at the render boundary (`benilla-assets`).
    pub position: [f32; 3],
}

/// One M2 **animation-event marker** (vanilla stride 44: `identifier 4CC @+0`, `data u32 @+4`,
/// `bone u32 @+8`, `position C3 @+12`, then a 20-byte enabled `M2TrackBase` we skip — its
/// per-sequence fire keys surface through `benilla-formats`' per-sequence event stream instead).
/// This is the record's **positional half**: the client's `0x7130e0`/`0x7131b0` scan exactly this
/// table by 4CC and transform `position` by `bone`'s live matrix — the cast-release launch points
/// `$CSL`/`$CSR`/`$CST` (casting hand left/right/two-hand), the bow anchors `$WTT`/`$WTB`, the
/// ranged release `$BWR`. Idents are NOT unique per model (character models carry six `$CSD`
/// records, one per voice payload); the client's queries take the first match. A record whose
/// `bone` doesn't fit `u16` or indexes past the bone array is dropped rather than kept malformed.
#[derive(Clone, Copy)]
pub struct M2EventMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    /// Model-space position, **raw WoW space** — same convention as [`M2Attachment::position`].
    pub position: [f32; 3],
}

/// One M2 **texture transform** (header `0x74`, file stride `0x54`): 3 back-to-back M2Tracks —
/// `translation` (C3Vector) @`+0x00`, `rotation` (quaternion) @`+0x1c`, `scaling` (C3Vector)
/// @`+0x38` (the M2 load `0x70ebd0`, runtime `[model+0x98]`). Sampled per frame these
/// drive UV animation — flowing waterfalls, scrolling energy fields. How the sampled TRS is
/// *applied* at draw (texture matrix vs CPU UVs, pivot/composition convention) is open; parsed
/// and carried here, not yet consumed.
pub struct M2TextureTransform {
    pub translation: M2Vec3Track,
    pub rotation: M2QuatTrack,
    pub scaling: M2Vec3Track,
}

/// One M2 **camera** record (header `cameras` @`0x124`, file stride `0x7c`) — the authored
/// eye/target path a `Cameras\*.m2` cinematic fly-by *is*, and the same record the portrait bake
/// and the `<Model>` panes frame themselves with.
///
/// Field map from the reference's M2 load `0x70ebd0` + the publish pass `0x718960`:
/// `type@+0x00 · fov@+0x04 · farClip@+0x08 · nearClip@+0x0c · positions@+0x10 ·
/// position_base@+0x2c · target@+0x38 · target_position_base@+0x54 · roll@+0x60`. The load copies
/// only fov/far/near into the runtime `CCamera`; the tracks are evaluated by the animate kernel and
/// **published against the bases** — `eye = position_base + positions(t)`,
/// `target = target_position_base + target(t)`, `roll = roll(t)` — so a keyless track leaves the
/// base as the value. Character and creature models author a single `(0,0,0)` key on all three
/// (their pane camera is a frozen constant); the fly-bys are the case the tracks exist for.
#[derive(Clone)]
pub struct M2Camera {
    /// The record's `type` word. `0` = portrait, `1` = "characterinfo" (the `<PlayerModel>` pane's
    /// raw index 1); every shipped `Cameras\*.m2` fly-by authors `-1`.
    pub camera_type: i32,
    /// **Diagonal** field of view, radians — the reference's camera build `0x7ac640` takes the
    /// half-angle as `θ = (fov/2)/√(aspect²+1)`, *not* `fov/2`. All ten fly-bys author `0.7854`
    /// (45°).
    pub fov: f32,
    /// Far clip as authored (`+0x08`). Every fly-by carries `27.777779` (= 1000/36).
    pub far_clip: f32,
    /// Near clip as authored (`+0x0c`). Every fly-by carries `0.22222222` (= 8/36).
    pub near_clip: f32,
    /// The eye path, relative to [`Self::position_base`].
    pub positions: M2Vec3SplineTrack,
    /// The eye's base position, model space (`+0x2c`).
    pub position_base: [f32; 3],
    /// The look-at path, relative to [`Self::target_base`].
    pub target: M2Vec3SplineTrack,
    /// The look-at base position, model space (`+0x54`).
    pub target_base: [f32; 3],
    /// Roll about the view axis, radians (`+0x60`).
    pub roll: M2ScalarSplineTrack,
}

/// The parsed model (render subset).
pub struct M2Model {
    pub vertices: Vec<M2Vertex>,
    pub textures: Vec<M2Texture>,
    pub materials: Vec<M2Material>,
    /// Per-`M2Color` **alpha** track (full [`M2ScalarTrack`]: interp + gseq tag + timed keys), one
    /// per color record (header `0x54`). A `texUnit.colorIndex` (direct) selects one; no keys ⇒ the
    /// factor doesn't apply. The alpha governs batch visibility (a constant 0 hides the batch) and,
    /// time-varying, the animated material combine (`0x707680`).
    pub color_alpha_tracks: Vec<M2ScalarTrack>,
    /// Per-`M2Color` **RGB colour** track (full [`M2Vec3Track`]: interp + gseq tag + timed
    /// `C3Vector` keys, the colour record's `+0x00` track), one per record. Selected by the same
    /// `texUnit.colorIndex`. This is the per-batch tint the real client multiplies into the vertex
    /// colour — load-bearing for additive glow cards drawn with a *neutral* glow texture (e.g.
    /// `GenericGlow_Alpha_128`), whose warmth lives entirely here; dropping it renders the glow
    /// grey/white. Time-varying tracks (e.g. a spell effect's white-hot flash cooling to red) carry
    /// their timestamps so the render side can animate the tint. No keys ⇒ no tint.
    pub color_rgb_tracks: Vec<M2Vec3Track>,
    /// Per-`M2TextureWeight` **weight** track (full [`M2ScalarTrack`]), one per record (header
    /// `0x64`). Selected via `texUnit.weight_combo_index → transparency_lookup → here` (two-hop).
    pub transparency_tracks: Vec<M2ScalarTrack>,
    /// The transparency lookup table (header `0xa4`, u16): `weight_combo_index` indexes this to reach
    /// [`Self::transparency_tracks`] (two-hop, `0x707b19`–`0x707b26`).
    pub transparency_lookup: Vec<u16>,
    /// The **texture-unit lookup** table (header `0x9c`, u16): `texture_coord_combo_index` (texUnit
    /// `+0x12`) indexes this to decide where a stage's texture coordinates come from. A value
    /// `< 3` names one of the vertex UV channels; **`>= 3` (real art: `0xffff`) is a *generated*
    /// environment coordinate** — the model authors no usable UVs for that stage and the runtime
    /// derives them from the view-space reflection vector. See
    /// [`M2Model::stage_is_env_mapped`].
    pub texture_unit_lookup: Vec<u16>,
    /// The texture-transform records (see [`M2TextureTransform`]), selected via
    /// `texture_transform_combo_index → texture_transform_lookup → here`.
    pub texture_transforms: Vec<M2TextureTransform>,
    /// The texture-animation lookup table (header `0xac`, u16): `texture_transform_combo_index`
    /// (texUnit `+0x16`, `0x70b897`) indexes this to reach
    /// [`Self::texture_transforms`]; `0xffff` = no animation.
    pub texture_transform_lookup: Vec<u16>,
    /// Global-sequence durations (header `0x14/0x18`, ms): the free-running loop clocks
    /// `gseq`-tagged tracks wrap on.
    pub global_sequences: Vec<u32>,
    pub bones: Vec<M2Bone>,
    /// The authored camera records (header `0x124`) — see [`M2Camera`]. Empty for the overwhelming
    /// majority of models; a `Cameras\*.m2` fly-by carries exactly one.
    pub cameras: Vec<M2Camera>,
    /// The **CameraLookup** table (header `0x12c`, u16): a camera *purpose* index → the slot in
    /// [`Self::cameras`] that serves it. The portrait bake selects through it (`cameraLookup[0]`,
    /// `0x525266`); the `<Model>` pane indexes [`Self::cameras`] raw instead (`0x76cec0`). Carried
    /// so a selection can be written the way the reference writes it rather than assumed 1:1.
    pub camera_lookup: Vec<u16>,
    pub raw_data: M2RawData,
    pub header: M2Header,
    /// Model-space **Z** of attachment id 17 — the reference's follow-camera pivot height
    /// (`0x50cbc0`: the camera targets `feet + (attach17.z + 0.0972)·scale`). `None` when the model
    /// has no slot-17 attachment (a non-character model → the camera falls back to the box).
    pub pivot_attach_z: Option<f32>,
    /// The full attachment-point table (see [`M2Attachment`]) — every record whose id/bone fit `u16`
    /// and whose bone indexes into [`Self::bones`]. `pivot_attach_z` above is the id-17 special case
    /// kept as its own field (the camera pivot's one historical consumer). **Records are addressed
    /// by ATTACH ID through [`Self::attachment`], never by scanning this table** (see
    /// [`Self::attach_lookup`]).
    pub attachments: Vec<M2Attachment>,
    /// The model's **AttachLookup** (header `+0x10c`), translated into [`Self::attachments`]
    /// indices: row `i` is the record attachment **id** `i` resolves to, `0xffff` for none. Read
    /// it through [`Self::attachment`], which is the reference's `0x710310` — bounds check and
    /// sentinel included.
    pub attach_lookup: Vec<u16>,
    /// The animation-event table's **positional markers** (see [`M2EventMarker`]) — every record
    /// whose bone fits `u16` and indexes into [`Self::bones`], in file order (queries take the
    /// first ident match, the client's `0x7130e0` scan order). The cast-release launch points
    /// (`$CSL`/`$CSR`/`$CST`), the ranged release (`$BWR`), the bow anchors, the sound/impact tags'
    /// anchor points.
    pub event_markers: Vec<M2EventMarker>,
    /// The model's **AnimationLookup** (header `+0x24`/`+0x28`): row `i` is the file slot of this
    /// model's first sequence for `AnimationData.dbc` id `i`, or `0xffff` where it authors none.
    /// This is the table behind the reference's ownership predicate — see [`M2Model::owns_animation`].
    /// Note the table is *short*: a model's `nAnimationLookup` stops just past the highest id it
    /// authors, so an id beyond the end reads as the out-of-bounds sentinel, i.e. **not owned**.
    pub animation_lookup: Vec<u16>,
    /// The model's baked **PlayableAnimationLookup** (decision 0082 — see [`M2PlayableAnim`]): row `i`
    /// is the model's own precomputed substitute for requested `AnimationData.dbc` id `i`. Empty for a
    /// version past 263 (the array is dropped) or a model whose header couldn't be read that far — the
    /// resolver degrades to identity in that case.
    pub playable_animation_lookup: Vec<M2PlayableAnim>,
    /// Embedded skin-profile array `(count, file_offset)` — `parse_embedded_skin` reads from here.
    pub(crate) views: (u32, u32),
    pub(crate) version: u32,
}

impl M2Model {
    /// **Are this batch's stage-`stage` texture coordinates GENERATED (environment-mapped) rather
    /// than read from the vertex UVs?**
    ///
    /// The reference's gate, at `0x70b8bd`
    /// (`cmp word[ [MD20+0xa0][texCoordSet + stage] ], 0x2; jbe`): a [`Self::texture_unit_lookup`]
    /// entry `<= 2` names a vertex UV channel, anything higher (real art authors `0xffff`) is a
    /// **generated environment coordinate**. Such a stage can never be `textureTransform`-driven,
    /// and the shipped vertex program writes it as the sphere-map remap of the view-space
    /// reflection vector — `uv = normalize(P − 2(P·N)N).xy · 0.5 + 0.5`, the `(0.5,0,0,0.5 /
    /// 0,0.5,0,0.5)` matrix at `0x70b8d0`.
    ///
    /// **An out-of-range index reads as env-mapped**, matching the reference: `0x70b8bd` indexes
    /// the table unguarded and falls through to the same `else` branch that writes the env remap.
    /// That is also the only safe direction — a model whose art relies on generated coordinates
    /// authors *no usable UVs at all* (`GnomeSubwayGlass.m2`: all 330 vertices at exactly
    /// `(0,0)`), so mistaking env for UV paints the whole mesh in a single corner texel.
    pub fn stage_is_env_mapped(&self, batch: &crate::SkinBatch, stage: u16) -> bool {
        let idx = batch.texture_coord_combo_index as usize + stage as usize;
        self.texture_unit_lookup.get(idx).is_none_or(|&v| v > 2)
    }

    /// **Does this model author `AnimationData.dbc` id `anim_id`?** — the reference's `0x711960`,
    /// byte-for-byte: a bounds-checked read of [`Self::animation_lookup`] (`0x710310` returns the
    /// `0xffff` sentinel for an out-of-range index) compared against `0xffff`.
    ///
    /// This is the predicate the GameObject animation arm branches on when the substate's animation
    /// id is missing from the model (the four-way remap in `0x5f3930`), so it
    /// must answer exactly as the reference does — including for an id past the table's end, which
    /// is **not owned** even if some later mechanism could reach a sequence carrying it.
    pub fn owns_animation(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// **The attachment record an attach id resolves to** — the reference's `0x710310` +
    /// `0x712f70` pair, byte-for-byte: a bounds-checked [`Self::attach_lookup`] read
    /// (`id >= count` ⇒ the `0xffff` sentinel) and then the record it names. `None` where the id
    /// has no record, which is how the reference behaves too: `0x712f70` stores the sentinel and
    /// every consumer of that field (`0x7140aa`, `0x718668`, `0x719266`) tests `cmp 0xffff; je`
    /// and skips the child, so an unresolvable id hangs **nothing** rather than falling back to
    /// the model origin.
    ///
    /// Never scan [`Self::attachments`] for a matching id instead: on the ~5% of weapon models
    /// that author duplicate ids the two disagree, and the lookup is the authority (decision
    /// 0805).
    pub fn attachment(&self, id: u16) -> Option<&M2Attachment> {
        let idx = *self.attach_lookup.get(id as usize)?;
        (idx != 0xffff)
            .then(|| self.attachments.get(idx as usize))
            .flatten()
    }
}

/// Holds the parsed model (mirrors the reference's `parse_m2(..).model()` shape).
pub struct M2Format {
    pub(crate) model: M2Model,
}
impl M2Format {
    pub fn model(&self) -> &M2Model {
        &self.model
    }
}
