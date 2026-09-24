//! The render types every model source (M2 batches, WMO groups) produces, one per render batch.

use super::key_anim::SeqLoops;
use super::mat_anim::{AlphaAnim, RgbAnim};
use super::tex_anim::UvAnim;

/// How a submesh blends: the model's blend mode mapped to the renderer's alpha handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelBlend {
    Opaque,
    AlphaTest,
    /// Alpha-blended or additive.
    Blend,
    /// `out = src·dst` (GL `DST_COLOR/ZERO`; `0x811fe0`: M2 mode 5 → EGxBlend 4; WMO MOMT mode 4).
    /// The equation reads no alpha, so these batches cannot alpha-fade.
    Mod,
    /// `out = 2·src·dst` (GL `DST_COLOR/SRC_COLOR`; M2 mode 6 → EGxBlend 5; WMO mode 5), neutral at
    /// mid-grey: the ARMORREFLECT sheen. No alpha either.
    Mod2x,
}

/// A character texture the client supplies at runtime (the record has a type, no filename):
/// - `Body` (type 1): the composited body atlas;
/// - `Hair` (type 6): CharSections `sectionType 3` `TextureName[0]` by hair style and colour;
/// - `Object` (type 2): the item's texture on a weapon or shield, the cape on a body;
/// - `SkinExtra` (type 8): CharSections `sectionType 0` `TextureName[1]` by skin colour,
///   uncomposited (fur).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharSkinSlot {
    Body,
    Hair,
    Object,
    SkinExtra,
}

/// How a billboard bone tracks the camera (M2 bone flags); a cylindrical kind keeps one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillboardKind {
    /// Spherical (`0x08`): faces the camera fully (glow cards, coronae).
    Spherical,
    /// Cylindrical lock-X (`0x10`): keeps the bone's X axis (chains, ropes).
    LockX,
    /// Cylindrical lock-Y (`0x20`).
    LockY,
    /// Cylindrical lock-Z (`0x40`): stays upright, turning to the viewer (the questgiver `?`).
    LockZ,
}

impl BillboardKind {
    /// The kind a bone's flags author; `0x08` wins over a lock bit, as the palette dispatch orders.
    pub fn from_bone_flags(bits: u32) -> Option<Self> {
        if bits & 0x08 != 0 {
            Some(Self::Spherical)
        } else if bits & 0x10 != 0 {
            Some(Self::LockX)
        } else if bits & 0x20 != 0 {
            Some(Self::LockY)
        } else if bits & 0x40 != 0 {
            Some(Self::LockZ)
        } else {
            None
        }
    }
}

/// Bone flags `0x1/0x2/0x4`, ignore parent translate/scale/rotate: for a non-root bone with
/// `flags & 7` (`0x714961`) the reference rebuilds the parent matrix from the model root, pivot
/// kept (`0x71496d..0x714d0c`), then billboards as usual. Legs: `0x1` at `0x714c92`, `0x2` at
/// `0x714bdb`, `0x4` at `0x714a6e` alone and `0x714a18` with `0x2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentArm {
    /// `0x1`: the frame sits at the model root's origin, not the animated parent's pivot.
    pub ignore_translate: bool,
    pub basis: ParentBasis,
}

/// The `flags & 0x6` leg: the parent's basis vectors (the reference's row K is our column K).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentBasis {
    /// `flags & 6 == 0`: the parent's basis as-is.
    Keep,
    /// `flags & 6 == 2`, ignore parent scale: each basis vector normalized.
    UnitNormalize,
    /// `flags & 6 == 4`, ignore parent rotation: root directions, parent lengths (`0x714a6e`).
    RootDirection,
    /// `flags & 6 == 6`: the root's basis (`0x714a18`). Every player mount's rider seat is this or
    /// [`Self::RootDirection`], so the saddle never rotates.
    RootBasis,
}

impl ParentArm {
    /// The arm a bone's flags author; `None` goes straight to the billboard selector (`0x714d0f`).
    pub fn from_bone_flags(bits: u32) -> Option<Self> {
        if bits & 0x7 == 0 {
            return None;
        }
        Some(Self {
            ignore_translate: bits & 0x1 != 0,
            basis: match bits & 0x6 {
                0x2 => ParentBasis::UnitNormalize,
                0x4 => ParentBasis::RootDirection,
                0x6 => ParentBasis::RootBasis,
                _ => ParentBasis::Keep,
            },
        })
    }
}

/// A bone scale track on a global sequence, the pulse a glow card rides (the lamppost's
/// `0.86..1.04` over 1333 ms), sampled at the scene clock less the instance's attach time, mod the
/// duration (`0x714352`).
#[derive(Debug, Clone)]
pub struct BoneScaleAnim {
    pub duration_ms: u32,
    /// Linear interpolation between keys (`interp != 0`); `false` steps.
    pub interp: bool,
    /// `(timestamp_ms, scale_xyz)`, time-ascending.
    pub keys: Vec<(u32, [f32; 3])>,
}

impl BoneScaleAnim {
    /// The scale at `time_ms` mod the duration, by the key search `0x713d50` and its linear leg.
    pub fn sample(&self, time_ms: u32) -> [f32; 3] {
        let n = self.keys.len();
        if n == 0 {
            return [1.0; 3];
        }
        let t = time_ms % self.duration_ms.max(1);
        if t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        let mut k = 0;
        while k + 1 < n && self.keys[k + 1].0 <= t {
            k += 1;
        }
        if !self.interp || k + 1 >= n {
            return self.keys[k].1; // step, or clamp past the final key
        }
        let (t0, v0) = self.keys[k];
        let (t1, v1) = self.keys[k + 1];
        let frac = if t1 > t0 {
            (t - t0) as f32 / (t1 - t0) as f32
        } else {
            0.0
        };
        [
            v0[0] + (v1[0] - v0[0]) * frac,
            v0[1] + (v1[1] - v0[1]) * frac,
            v0[2] + (v1[2] - v0[2]) * frac,
        ]
    }
}

/// A parentless bone whose armed sequence keys rotation only: geometry wholly on it turns rigidly,
/// `T(pivot) · R(t) · T(−pivot)`, with no palette (`CavernsOfTimeSky.m2`'s asteroid belts). Whole
/// weighting is the caller's check.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneSpin {
    /// The rotation centre, WoW model space.
    pub pivot: [f32; 3],
    /// The armed sequence's length in seconds.
    pub duration: f32,
    /// `false` steps (`interp_type == 0`), read from the track header as `BoneKeys` lacks it.
    pub interp: bool,
    /// `(seconds, [x, y, z, w])`, time-ascending, raw WoW model space.
    pub keys: Vec<(f32, [f32; 4])>,
}

impl BoneSpin {
    /// The rotation at `time` mod the duration, slerped as [`BoneScaleAnim::sample`] lerps. Past
    /// the last key it clamps, so a loop snaps back as authored (`CavernsOfTimeSky` bone 1).
    pub fn sample(&self, time: f32) -> [f32; 4] {
        const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
        let n = self.keys.len();
        if n == 0 {
            return IDENTITY;
        }
        // `rem_euclid`, not `%`: a cursor read before the anchor must land inside the loop.
        let t = if self.duration > 0.0 {
            time.rem_euclid(self.duration)
        } else {
            0.0
        };
        if t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        let mut k = 0;
        while k + 1 < n && self.keys[k + 1].0 <= t {
            k += 1;
        }
        if !self.interp || k + 1 >= n {
            return self.keys[k].1; // step, or clamp past the final key
        }
        let (t0, q0) = self.keys[k];
        let (t1, q1) = self.keys[k + 1];
        let frac = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        slerp(q0, q1, frac)
    }
}

/// Shortest-arc slerp on plain arrays (`benilla-formats` carries no math crate).
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = (0..4).map(|i| a[i] * b[i]).sum::<f32>();
    let mut b = b;
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }
    let (w0, w1) = if dot > 0.9995 {
        (1.0 - t, t) // near-parallel: lerp, then renormalise below
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    let mut q = [0.0f32; 4];
    for i in 0..4 {
        q[i] = a[i] * w0 + b[i] * w1;
    }
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 0.0 {
        for c in &mut q {
            *c /= len;
        }
    }
    q
}

/// A batch on an M2 billboard bone, turned to the camera about `pivot` (WoW model space).
#[derive(Debug, Clone)]
pub struct Billboard {
    pub pivot: [f32; 3],
    /// The joint the card rides on an animated host (a swinging lamp).
    pub bone: u16,
    pub kind: BillboardKind,
    pub scale_anim: Option<BoneScaleAnim>,
    /// Translation loops `(anim id, band keys)`: anim 0 the questgiver bob, 190 its raised form
    /// under an overhead name (`0x6076c0`). Only the marker spawn site arms them.
    pub seq_translations: Vec<(u16, BoneScaleAnim)>,
}

/// A WMO group batch's MOBA section (the TRANS, INT, EXT runs of MOGP `+0x28/+0x2a/+0x2c`). In an
/// interior group INT draws unlit `tex × MOCV`, TRANS lerps by MOCV alpha from the lit surface to
/// that bake, and EXT is lit as outdoors, as captures of the reference show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WmoBatchClass {
    Trans,
    Int,
    Ext,
}

/// The fog colour of the batch state setter (`0x70baf0`, table `DAT_811fc4` at `0x70bddf`): the
/// scene's, black for Add, white for Mod, grey for Mod2x, off under render flag 0x02 (`0x70bb24`).
/// Discriminants are the shader encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FogPolicy {
    #[default]
    Scene = 0,
    Black = 1,
    White = 2,
    Grey = 3,
    Off = 4,
}

/// One render batch: self-contained geometry and its material.
#[derive(Debug, Clone)]
pub struct RenderSubmesh {
    pub positions: Vec<[f32; 3]>,
    /// Authored normals: WoW lights foliage by these soft normals, where flat ones light crossed
    /// canopies harshly. Empty if the source has none, and the renderer computes them.
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// `None` for an unfilled creature skin slot ([`Self::skin_slot`]).
    pub texture: Option<String>,
    /// `Monster1/2/3` as `Some(0/1/2)`, for a skin-less load to fill at spawn.
    pub skin_slot: Option<u8>,
    /// The `skinSectionId` (`group*100 + variant`) the character compositor shows geosets by.
    pub geoset_id: u16,
    pub char_slot: Option<CharSkinSlot>,
    pub blend: ModelBlend,
    /// Address mode from the texture record (`0x1` repeat U, `0x2` repeat V, else clamp): a cutout
    /// card's margin must clamp to its transparent border, not wrap into the opaque middle. WMO
    /// always repeats.
    pub wrap_x: bool,
    pub wrap_y: bool,
    /// M2 material flag `0x04` or MOMT UNCULLED `0x04`; without it the reference culls back faces.
    pub two_sided: bool,
    /// Per-vertex skin: 4 global M2 bone indices, used as joint indices; empty for WMO.
    pub joints: Vec<[u16; 4]>,
    /// Summing to 1, parallel to [`Self::joints`].
    pub weights: Vec<[f32; 4]>,
    /// Per-vertex colour. WMO: the MOCV bake, modulating light outdoors and being the light
    /// indoors ([`Self::interior`]), its alpha lighting data on interior TRANS/INT batches and 1
    /// elsewhere. M2: the constant M2Color tint. Empty is untinted.
    pub vertex_colors: Vec<[f32; 4]>,
    /// A WMO interior-group batch: with neither exterior bit of `groupFlags & 0x48` (`0x6b3f90`),
    /// lit by its MOCV with the sun off. The combine is matched to captures; its gx layer is
    /// untraced.
    pub interior: bool,
    /// Unlit: M2 material flag `0x01`, or MOMT `0x01` on exterior WMO batches only; the interior
    /// drawer (`0x6b5190`) ignores it and lights by section, the exterior one (`0x6b4f10`) per
    /// material.
    pub emissive: bool,
    /// Texture type 14, filled by `Model:ReplaceIconTexture` (`0x710ec0`), as on the bag buttons'
    /// item-push card `ForcedBackpackItem.m2`.
    pub icon_slot: bool,
    /// MOMT SIDN (`0x10`) colour, RGB gamma bytes: the windows' night glow. The reference scales it
    /// by the night fraction (ramping 20:30→21:30 and 06:00→07:00) into the GL emission inside the
    /// lit sum, `tex × (lit + sidn·night)`, so lit batches only (`0x6b4090`).
    pub sidn: Option<[u8; 3]>,
    /// MOMT WINDOW (`0x20`): the interior drawer lights the batch by the midpoint of the Direct and
    /// Ambient bands (ambient +16/255), per batch (`0x6d37e0`).
    pub window: bool,
    /// M2 blend mode 3 or 4 (glow cards); alpha-blended instead, the background mutes the hue.
    pub additive: bool,
    /// M2 render flag `0x10`; without it the reference writes depth for every batch (`0x70c190`).
    pub no_depth_write: bool,
    /// M2 render flag `0x08`: drawn over everything.
    pub no_depth_test: bool,
    pub fog_policy: FogPolicy,
    pub billboard: Option<Billboard>,
    /// Geometry on a billboard bone the card split refused: the reference blends it per vertex, so
    /// only a skinned draw bends it.
    pub welded_billboard: bool,
    /// Time-varying colour-alpha and transparency-weight loops ([`mat_anim`](super::mat_anim)),
    /// multiplied into the render alpha as the reference combines them (`0x707680`).
    pub alpha_anim: Option<AlphaAnim>,
    /// The texture transform's translation loop ([`tex_anim`](super::tex_anim)), raw `(x, y)`.
    pub uv_anim: Option<UvAnim>,
    /// The UV loop per file sequence slot, only when the slots disagree (BRM's lava bubbles key
    /// their flipbook in variation 1, leaving slot 0 a dead hold).
    pub uv_seq: Option<SeqLoops<[f32; 2]>>,
    /// The texture transform's rotation per slot, for lanes owning a material per instance (the UI
    /// model tiles, the cooldown sweep).
    pub uv_rot_seq: Option<SeqLoops<[f32; 4]>>,
    /// The scaling loop per slot, as [`Self::uv_rot_seq`].
    pub uv_scale_seq: Option<SeqLoops<[f32; 2]>>,
    /// The time-varying M2Color tint ([`mat_anim`](super::mat_anim)), carried on the material; the
    /// static vertex tint is then skipped so the two never double-apply.
    pub rgb_anim: Option<RgbAnim>,
    /// The tint counterpart of [`Self::uv_seq`].
    pub rgb_seq: Option<SeqLoops<[f32; 3]>>,
    pub wmo_batch: Option<WmoBatchClass>,
    /// The M2 skin section drawn. Two batches on one section rasterize the same triangles (a sheen
    /// over its base); the reference draws both from one vertex array under LEQUAL, the later
    /// winning exactly (`0x70c190`), so consolidators refuse a section they cannot take whole.
    pub section: Option<u16>,
    /// Texture coordinates generated as a view-space sphere map, not read from [`Self::uvs`]
    /// (`texture_unit_lookup[texCoordSet] > 2`): `uv = normalize(P − 2(P·N)N).xy · 0.5 + 0.5`.
    /// Such a mesh parks its UVs at one point (`GnomeSubwayGlass.m2`).
    pub env_map: bool,
}

/// A flat ground-plane quad: four vertices on one horizontal plane at or just above z = 0 (hover
/// ≤ [`GROUND_HOVER_MAX`]), an axis-aligned XY rectangle wholly on one bone, as ground-ring spells
/// author them (Battle Shout's crescents). The reference draws them as geometry slopes bury.
/// Deviation: the fx lane redraws them as projected decals (`ground_fx`) so they drape the terrain.
#[derive(Debug, Clone, Copy)]
pub struct GroundQuad {
    /// Posed through the joint's matrix × the bone's inverse bind pose, as a skinned vertex.
    pub bone: u16,
    /// WoW model space, in bilinear order: `(min_x, min_y)`, `(max_x, min_y)`, `(min_x, max_y)`,
    /// `(max_x, max_y)`.
    pub corners: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
    /// The static M2Color tint, white when none: a decal redraws the quad from its corners, and
    /// spell ground art on neutral `GENERICGLOW*` radials has no other colour.
    pub tint: [f32; 3],
}

/// Vertices further than this from the quad's plane disqualify it; the art authors exact planes.
const GROUND_FLAT_EPS: f32 = 0.01;

/// The highest plane z still ground-plane: hovering discs sit at 0.014..0.518, and the next plane,
/// `GreaterHeal_Low_Base`'s mid-body glow at 1.389, must not drape.
const GROUND_HOVER_MAX: f32 = 1.0;

/// The empty batch, for test scaffolding: nothing authored, `Opaque`, repeat sampling.
impl Default for RenderSubmesh {
    fn default() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            texture: None,
            skin_slot: None,
            geoset_id: 0,
            char_slot: None,
            blend: ModelBlend::Opaque,
            wrap_x: true,
            wrap_y: true,
            two_sided: false,
            joints: Vec::new(),
            weights: Vec::new(),
            vertex_colors: Vec::new(),
            interior: false,
            emissive: false,
            icon_slot: false,
            sidn: None,
            window: false,
            additive: false,
            no_depth_write: false,
            no_depth_test: false,
            fog_policy: FogPolicy::Scene,
            billboard: None,
            welded_billboard: false,
            alpha_anim: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            rgb_anim: None,
            rgb_seq: None,
            wmo_batch: None,
            env_map: false,
            section: None,
        }
    }
}

impl RenderSubmesh {
    /// A billboard card authored back to front: one plane with its normal in the −X half-space,
    /// away from the viewer the billboard arm aims bone-local +X at (`0x71547c`; M2 bones have no
    /// bind rotation, so bone-local is model space). The reference skins that normal unflipped
    /// through the same palette row (`0x71a460`). Deviation: consumers light such a card off the
    /// side it presents, because the reference's shading of it is inverted and follows the camera.
    pub fn billboard_card_faces_away(&self) -> bool {
        self.billboard.is_some() && self.plane_normal().is_some_and(|n| n[0] < -Self::EDGE_ON_X)
    }

    /// A normal with |x| under this is edge-on to the camera axis: no facing to decide.
    const EDGE_ON_X: f32 = 1e-3;

    /// The one plane every authored normal shares; `None` for 3-D geometry (the questgiver `?`),
    /// which has no single facing.
    pub fn plane_normal(&self) -> Option<[f32; 3]> {
        /// Cosine floor for "same plane", allowing for soft-averaged normals.
        const SAME_PLANE_COS: f32 = 0.999;
        let unit = |n: &[f32; 3]| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (l > 1e-6).then(|| [n[0] / l, n[1] / l, n[2] / l])
        };
        let n0 = self.normals.first().and_then(unit)?;
        self.normals
            .iter()
            .all(|n| {
                unit(n).is_some_and(|n| n[0] * n0[0] + n[1] * n0[1] + n[2] * n0[2] > SAME_PLANE_COS)
            })
            .then_some(n0)
    }

    /// The flat ground-plane quad, strictly: other flat spell batches (Fist of Justice, the IceNuke
    /// impact) stay on the ordinary path.
    pub fn ground_quad(&self) -> Option<GroundQuad> {
        self.ground_quad_hover(GROUND_HOVER_MAX).map(|(q, _)| q)
    }

    /// [`Self::ground_quad`] under any hover ceiling, with the plane's z (for `groundscan`).
    pub fn ground_quad_hover(&self, max_hover: f32) -> Option<(GroundQuad, f32)> {
        if self.billboard.is_some()
            || self.positions.len() != 4
            || self.joints.len() != 4
            || self.weights.len() != 4
            || self.uvs.len() != 4
        {
            return None;
        }
        // One horizontal plane; the corners keep their authored z, as the decal drapes them anyway.
        let hover = self.positions.iter().map(|p| p[2]).sum::<f32>() / 4.0;
        if !(-GROUND_FLAT_EPS..=max_hover).contains(&hover)
            || self
                .positions
                .iter()
                .any(|p| (p[2] - hover).abs() > GROUND_FLAT_EPS)
        {
            return None;
        }
        // One bone, full weight, across all four vertices.
        let bone = self.joints[0][0];
        if self
            .joints
            .iter()
            .zip(&self.weights)
            .any(|(j, w)| j[0] != bone || w[0] < 0.999)
        {
            return None;
        }
        // An axis-aligned rectangle: every vertex on a corner of the XY bounding box.
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &self.positions {
            for a in 0..2 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        let eps = [
            (max[0] - min[0]) * 1e-3 + 1e-6,
            (max[1] - min[1]) * 1e-3 + 1e-6,
        ];
        if max[0] - min[0] <= eps[0] || max[1] - min[1] <= eps[1] {
            return None; // degenerate (zero-area) quad
        }
        // Slot each vertex into its bilinear rect position; every slot must fill exactly once.
        let mut corners = [[0.0_f32; 3]; 4];
        let mut uvs = [[0.0_f32; 2]; 4];
        let mut filled = [false; 4];
        for (p, uv) in self.positions.iter().zip(&self.uvs) {
            let sx = if (p[0] - min[0]).abs() <= eps[0] {
                0
            } else if (p[0] - max[0]).abs() <= eps[0] {
                1
            } else {
                return None; // an x between the extremes: not a corner
            };
            let sy = if (p[1] - min[1]).abs() <= eps[1] {
                0
            } else if (p[1] - max[1]).abs() <= eps[1] {
                1
            } else {
                return None;
            };
            let slot = sy * 2 + sx;
            if filled[slot] {
                return None; // two vertices on one corner
            }
            filled[slot] = true;
            corners[slot] = *p;
            uvs[slot] = *uv;
        }
        // The constant M2Color off the vertex bake, which all four share; white when none.
        let tint = self
            .vertex_colors
            .first()
            .map_or([1.0; 3], |c| [c[0], c[1], c[2]]);
        Some((
            GroundQuad {
                bone,
                corners,
                uvs,
                tint,
            },
            hover,
        ))
    }
}
