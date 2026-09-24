//! The parsed M2 record types and the [`M2Model`] that `crate::parse_m2` builds.

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

/// M2 texture type: `Hardcoded` names its file; `Monster*` slots take the creature display's skins.
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

/// A texture's filename.
pub struct M2ArrayString {
    pub string: CString,
}

/// One M2 texture definition.
pub struct M2Texture {
    pub texture_type: M2TextureType,
    /// Address mode on U (`flags & 0x1`): set repeats, clear clamps to edge.
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

/// One M2 bone: keyBoneId i32 @+0x00, flags @+0x04, parent i16 @+0x08, pivot C3 @+0x60.
pub struct M2Bone {
    /// `KeyBoneID`, `-1` for none: 0/1 arms L/R, 2/3 shoulders L/R, 4 spine low, 5 waist, 6 head…
    pub key_bone: i16,
    pub flags: M2BoneFlags,
    /// Parent bone index, `-1` for a root.
    pub parent: i16,
    pub pivot: C3,
}
impl M2Bone {
    /// Whether this bone billboards (any of the spherical/cylindrical bits).
    pub fn is_billboard(&self) -> bool {
        self.flags.0 & (0x08 | 0x10 | 0x20 | 0x40) != 0
    }
}

/// One M2 vertex, 48 bytes: position @0, weights @0x0c, indices @0x10, normal @0x14, UV @0x20,
/// and an unread second UV @0x28.
pub struct M2Vertex {
    pub position: C3,
    /// Per-vertex bone influence weights (`/255`), paired with [`Self::bone_indices`].
    pub bone_weights: [u8; 4],
    pub bone_indices: [u8; 4],
    pub normal: C3,
    pub tex_coords: C2,
}

/// The texture lookup table and the collision hull's raw bytes.
pub struct M2RawData {
    pub texture_lookup_table: Vec<u16>,
    /// Collision-hull triangle indices, raw bytes (`count × 2`, `u16` each).
    pub bounding_triangles: Vec<u8>,
    /// Collision-hull vertices, raw bytes (`count × 12`, `C3Vector` each).
    pub bounding_vertices: Vec<u8>,
}

/// The MD20 header's authored bounds: the bounding box and sphere are the render and cull volume
/// over every animation, the collision box and sphere the tight hull.
pub struct M2Header {
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub collision_box_min: [f32; 3],
    pub collision_box_max: [f32; 3],
    pub collision_sphere_radius: f32,
}

/// A PlayableAnimationLookup row (`0x711bf0`, header `+0x2c`): the id this model plays for a
/// requested `AnimationData.dbc` id, and a direction code. Every 1.12.1 model has 203 rows.
#[derive(Clone, Copy)]
pub struct M2PlayableAnim {
    /// The `AnimationData.dbc` id this model plays for the row's requested id.
    pub resolved_id: u16,
    /// The direction or variant playback code (near `0x7126d2`); parsed, not applied: the
    /// reference's use of it is untraced.
    pub dir_flags: u16,
}

/// One attachment point: a 48-byte vanilla record, `id` and `bone` narrowed to `u16`. On character
/// models `position` equals the attach bone's pivot. Ids: 0 shield (left forearm), 1 right hand,
/// 2 left hand, 26/27 right/left back sheath, 28 shield back, 30/31 lower back, 32/33 hips.
#[derive(Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    /// Model-space position, raw WoW axes like [`M2Bone::pivot`].
    pub position: [f32; 3],
}

/// The positional half of an animation-event record: the reference (`0x7130e0`, `0x7131b0`) finds
/// one by 4CC, first match, and moves `position` by `bone`'s matrix. Cast launch points
/// `$CSL`/`$CSR`/`$CST`, bow anchors `$WTT`/`$WTB`, ranged release `$BWR`.
#[derive(Clone, Copy)]
pub struct M2EventMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    /// Model-space position, raw WoW axes.
    pub position: [f32; 3],
}

/// One M2 texture transform (header `0x74`, stride `0x54`, loaded at `0x70ebd0`), its UV-animation
/// tracks at `+0x00`, `+0x1c`, `+0x38`; this crate only carries the parsed tracks.
pub struct M2TextureTransform {
    pub translation: M2Vec3Track,
    pub rotation: M2QuatTrack,
    pub scaling: M2Vec3Track,
}

/// One M2 camera record (header `0x124`, stride `0x7c`, loaded at `0x70ebd0`): a `Cameras\*.m2`
/// fly-by's path, and the framing of the portrait bake and `<Model>` panes. The publish pass
/// `0x718960` adds each track to its base (`eye = position_base + positions(t)`), so a keyless
/// track leaves the base.
#[derive(Clone)]
pub struct M2Camera {
    /// The `type` word (`+0x00`): 0 portrait, 1 the `<PlayerModel>` pane, `-1` on every fly-by.
    pub camera_type: i32,
    /// Diagonal field of view, radians (`+0x04`): `0x7ac640` takes the half-angle as
    /// `(fov/2)/√(aspect²+1)`, not `fov/2`.
    pub fov: f32,
    /// Far clip (`+0x08`); every fly-by carries `27.777779` (1000/36).
    pub far_clip: f32,
    /// Near clip (`+0x0c`); every fly-by carries `0.22222222` (8/36).
    pub near_clip: f32,
    /// The eye path (`+0x10`), relative to [`Self::position_base`].
    pub positions: M2Vec3SplineTrack,
    /// The eye's base position, model space (`+0x2c`).
    pub position_base: [f32; 3],
    /// The look-at path (`+0x38`), relative to [`Self::target_base`].
    pub target: M2Vec3SplineTrack,
    /// The look-at base position, model space (`+0x54`).
    pub target_base: [f32; 3],
    /// Roll about the view axis, radians (`+0x60`).
    pub roll: M2ScalarSplineTrack,
}

/// The parsed model.
pub struct M2Model {
    pub vertices: Vec<M2Vertex>,
    pub textures: Vec<M2Texture>,
    pub materials: Vec<M2Material>,
    /// Each `M2Color`'s alpha track (header `0x54`), picked directly by a batch's `colorIndex`; no
    /// keys means no factor. It gates visibility and feeds the material combine (`0x707680`).
    pub color_alpha_tracks: Vec<M2ScalarTrack>,
    /// Each `M2Color`'s RGB track: the batch tint multiplied into the vertex colour, none without
    /// keys. Neutral glow cards (`GenericGlow_Alpha_128`) take all their colour from it.
    pub color_rgb_tracks: Vec<M2Vec3Track>,
    /// Each `M2TextureWeight`'s weight track (header `0x64`), via [`Self::transparency_lookup`].
    pub transparency_tracks: Vec<M2ScalarTrack>,
    /// The transparency lookup (header `0xa4`, u16): `weight_combo_index` to
    /// [`Self::transparency_tracks`] (`0x707b19`–`0x707b26`).
    pub transparency_lookup: Vec<u16>,
    /// The texture-unit lookup (header `0x9c`), by `texture_coord_combo_index`: a UV channel or a
    /// generated environment coordinate (see [`Self::stage_is_env_mapped`]).
    pub texture_unit_lookup: Vec<u16>,
    /// The texture transforms, reached through [`Self::texture_transform_lookup`].
    pub texture_transforms: Vec<M2TextureTransform>,
    /// The texture-animation lookup (header `0xac`, u16): `texture_transform_combo_index`
    /// (`0x70b897`) to [`Self::texture_transforms`], `0xffff` for none.
    pub texture_transform_lookup: Vec<u16>,
    /// Global-sequence durations (header `0x14`, ms): the clocks `gseq` tracks wrap on.
    pub global_sequences: Vec<u32>,
    pub bones: Vec<M2Bone>,
    /// The camera records (header `0x124`): one in a `Cameras\*.m2` fly-by, none in most models.
    pub cameras: Vec<M2Camera>,
    /// CameraLookup (header `0x12c`): a camera purpose to its [`Self::cameras`] slot. The portrait
    /// bake reads it (`0x525266`); the `<Model>` pane indexes cameras directly (`0x76cec0`).
    pub camera_lookup: Vec<u16>,
    pub raw_data: M2RawData,
    pub header: M2Header,
    /// Model-space Z of attachment id 17, the follow camera's pivot height (`0x50cbc0` targets
    /// `feet + (z + 0.0972)·scale`).
    pub pivot_attach_z: Option<f32>,
    /// The surviving attachment records; address them by id via [`Self::attachment`], never scan.
    pub attachments: Vec<M2Attachment>,
    /// AttachLookup (header `+0x10c`) as [`Self::attachments`] indices, `0xffff` for none.
    pub attach_lookup: Vec<u16>,
    /// The animation-event markers in file order; a query takes the first ident match (`0x7130e0`).
    pub event_markers: Vec<M2EventMarker>,
    /// AnimationLookup (header `+0x24`): row `i` is the first sequence slot for
    /// `AnimationData.dbc` id `i`, `0xffff` for none; it ends past the highest authored id.
    pub animation_lookup: Vec<u16>,
    /// The PlayableAnimationLookup: row `i` is the model's substitute for requested id `i`.
    pub playable_animation_lookup: Vec<M2PlayableAnim>,
    /// The embedded skin profiles' `(count, file_offset)`.
    pub(crate) views: (u32, u32),
    pub(crate) version: u32,
}

impl M2Model {
    /// Whether `batch`'s stage `stage` takes generated environment coordinates: the reference's
    /// gate `0x70b8bd` treats a [`Self::texture_unit_lookup`] entry above 2 (art: `0xffff`), and an
    /// out-of-range index, as the sphere map of the view-space reflection (`0x70b8d0`), which no
    /// texture transform drives.
    pub fn stage_is_env_mapped(&self, batch: &crate::SkinBatch, stage: u16) -> bool {
        let idx = batch.texture_coord_combo_index as usize + stage as usize;
        self.texture_unit_lookup.get(idx).is_none_or(|&v| v > 2)
    }

    /// Whether the model authors `AnimationData.dbc` id `anim_id`: the reference's `0x711960`, a
    /// bounds-checked [`Self::animation_lookup`] read, so an id past the end is not owned.
    pub fn owns_animation(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// The record attach id `id` resolves to through [`Self::attach_lookup`] (`0x710310`,
    /// `0x712f70`). An unresolved id hangs nothing: its users (`0x7140aa`, `0x718668`, `0x719266`)
    /// skip the sentinel. A scan of [`Self::attachments`] disagrees where weapons repeat ids.
    pub fn attachment(&self, id: u16) -> Option<&M2Attachment> {
        let idx = *self.attach_lookup.get(id as usize)?;
        (idx != 0xffff)
            .then(|| self.attachments.get(idx as usize))
            .flatten()
    }
}

/// Holds the parsed model.
pub struct M2Format {
    pub(crate) model: M2Model,
}
impl M2Format {
    pub fn model(&self) -> &M2Model {
        &self.model
    }
}
